// @ts-nocheck
import { nextTick } from 'vue';
// Nonreactive xterm and WebSocket handles retain the original playback semantics.
export function createRecordsController(ui, elements) {
  const handlers = {};
  const $ = id => elements[id];
  const base = new URL('.', location.href);
  const params = new URLSearchParams(location.search);
  const node = params.get('node');
  const embedded = params.get('embedded') === '1' || window.self !== window.top;
  // 中央站：`nodes=a,b,c` 是所有勾选的机器，列表按机器聚合；单机页面 nodes = [null]。
  // Hub pages reach a node through /api/nodes/{nid}/api/{*path}.
  const nodes = (params.get('nodes') || '').split(',').filter(Boolean);
  if (!nodes.length) nodes.push(node || null);
  const nodeNames = new Map();
  const prefixFor = nid => (nid ? ['api', 'nodes', encodeURIComponent(nid), 'api', ''].join('/') : 'api/');
  const fitKey = SessionDockCapabilities.namespace + 'records-fit';
  const GAP_NOTE = '（录制有缺口，已从下一个快照继续）';
  const state = ui.state;

  let records = [];
  let term = null, fitAddon = null, socket = null;
  let generation = 0, reconnectTimer = 0, reconnectDelay = 1000, leaving = false;
  let fitOn = false, recordedCols = 0, recordedRows = 0;
  let loading = false;

  globalThis.__records = {term: () => term, socket: () => socket, state};

  ui.machine = node || location.hostname;
  // 嵌在主页面对话框里时外层已有标题栏，省掉自己的。
  if (embedded) document.body.classList.add('embedded');

  if (SessionDockCapabilities.config.terminal_records === false
      || SessionDockCapabilities.config.terminal === false) {
    setStatus('此机器未启用终端录制');
    return handlers;
  }

  try { fitOn = localStorage.getItem(fitKey) === 'true'; } catch {}
  ui.fitOn = fitOn;
  ui.empty = true;

  function apiURL(path, query, nid = nodes[0]) {
    const url = new URL(prefixFor(nid) + path, base);
    if (query) for (const [key, value] of Object.entries(query)) url.searchParams.set(key, value);
    return url;
  }
  function attachURL(id, nid) {
    const url = apiURL('term/records/attach', {id}, nid);
    url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    return url.href;
  }
  function setStatus(message, error = false) {
    state.status = message;
    ui.error = error;
  }
  function appendStatus(note) {
    if (!state.status.includes(note)) setStatus(state.status + note);
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
    const liveOnly = ui.liveOnly;
    return records.filter(row => !liveOnly || row.live);
  }
  function syncURL(id, nid) {
    const url = new URL(location.href);
    if (id) url.searchParams.set('id', id);
    else url.searchParams.delete('id');
    if (nid) url.searchParams.set('node', nid);
    history.replaceState(history.state, '', url);
  }
  function rowOf(id, nid) {
    return records.find(row => row.id === id && (row.node || null) === (nid || null))
      || records.find(row => row.id === id) || null;
  }
  function renderList() {
    ui.records = records;
    ui.nodeNames = Object.fromEntries(nodeNames);
  }
  async function loadList() {
    if (loading) return;
    loading = true;
    try {
      if (nodes[0] && !nodeNames.size) {
        try {
          const meta = await (await fetch(new URL('api/nodes', base))).json();
          for (const row of meta.machines || meta.nodes || []) if (row?.id) nodeNames.set(row.id, row.name || row.id);
          ui.machine = nodes.map(nid => nodeNames.get(nid) || nid).join(' · ');
        } catch { /* 机器名只是装饰 */ }
      }
      // 每台机器各自请求；一台失败不影响其它机器，错误合并到状态栏。
      const settled = await Promise.allSettled(nodes.map(async nid => {
        const response = await fetch(apiURL('term/records', null, nid), {cache: 'no-store'});
        if (!(response.headers.get('Content-Type') || '').includes('application/json'))
          throw new Error('无法读取响应，请确认登录状态后刷新');
        const result = await response.json();
        if (!response.ok) throw new Error(result.error || `请求失败（${response.status}）`);
        return (Array.isArray(result.records) ? result.records : []).map(row => ({...row, node: nid}));
      }));
      const failures = settled.filter(item => item.status === 'rejected').map(item => item.reason?.message || '失败');
      if (failures.length === nodes.length) throw new Error(failures[0]);
      records = settled.flatMap(item => item.status === 'fulfilled' ? item.value : [])
        .sort((a, b) => (b.created_ms || 0) - (a.created_ms || 0));
      renderList();
      if (!state.id && !state.status) setStatus(records.length ? '' : '没有录制');
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
    ui.fitOn = fitOn;
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
      ui.size = recordedCols && recordedRows ? `${recordedCols}×${recordedRows}` : '';
      setStatus(state.live ? '进行中 · 实时跟随' : '已结束');
      return;
    }
    if (msg.t === 'resize') {
      recordedCols = msg.cols || recordedCols;
      recordedRows = msg.rows || recordedRows;
      ui.size = recordedCols && recordedRows ? `${recordedCols}×${recordedRows}` : '';
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
    const ws = new WebSocket(attachURL(id, state.node || null));
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
  function openRecord(id, nid = rowOf(id)?.node || nodes[0]) {
    if (!id) return;
    const key = (nid || '') + '/' + id;
    const switching = state.key !== key;
    state.id = id;
    state.key = key;
    state.node = nid || '';
    if (switching) {
      state.status = '';
      state.live = false;
      state.ended = false;
      state.gaps = 0;
      state.bytes = 0;
      recordedCols = 0;
      recordedRows = 0;
      reconnectDelay = 1000;
      ui.size = '';
      setStatus('正在连接…');
      createTerm();
    }
    ui.empty = false;
    syncURL(id, nid);
    if (!switching && socket && socket.readyState <= WebSocket.OPEN) return;
    connect(id);
  }

  handlers.openRecord = openRecord;
  handlers.records_keydown = event => {
    const items = visibleRecords();
    if (!items.length) return;
    const current = items.findIndex(item => item.id === state.id);
    const go = index => {
      event.preventDefault();
      const next = items[Math.max(0, Math.min(items.length - 1, index))];
      if (next) openRecord(next.id, next.node || null);
    };
    if (event.key === 'ArrowDown') go(current < 0 ? 0 : current + 1);
    else if (event.key === 'ArrowUp') go(current < 0 ? 0 : current - 1);
    else if (event.key === 'Home') go(0);
    else if (event.key === 'End') go(items.length - 1);
    else if (event.key === 'Enter' || event.key === ' ') go(current < 0 ? 0 : current);
  };
  handlers.refresh_click = () => loadList();
  handlers.fit_click = async () => {
    fitOn = !fitOn;
    ui.fitOn = fitOn;
    try { localStorage.setItem(fitKey, String(fitOn)); } catch {}
    // Apply the original sizing after Vue has committed the fit class.
    await nextTick();
    applyRecordedSize();
  };
  handlers.copy_click = () => {
    if (!term) return;
    const text = term.getSelection();
    if (!text) return;
    navigator.clipboard.writeText(text);
  };
  handlers.bottom_click = () => { term?.scrollToBottom(); };
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
  loadList().then(() => { if (wanted) openRecord(wanted, node || null); });

return handlers;
}
