import {LineDecoder, encodeResize} from './grid/wire.js';
import {GridModel} from './grid/model.js';
import {GridRenderer} from './grid/render.js';
import {InputEncoder, KeyCapture} from './grid/input.js';

const $ = id => document.getElementById(id);
const PAGE_ID = crypto.randomUUID();
const encoderUtf8 = new TextEncoder();
const base = new URL('.', location.href);
const params = new URLSearchParams(location.search);
const node = params.get('node');
const wantedName = params.get('name');
// `?record=<id>`：只读回放一段录制（同一模型、同一渲染器），不 claim、不发输入。
const wantedRecord = params.get('record');
const readOnly = !!wantedRecord;
const prefix = node ? ['api', 'nodes', encodeURIComponent(node), 'api', ''].join('/') : 'api/';

const state = {
  name: '',
  status: '',
  connected: false,
  cols: 0,
  rows: 0,
  following: true,
  selection: null,
  lastSeq: 0,
  readOnly,
  record: wantedRecord || '',
  live: false,
  ended: false,
};

const model = new GridModel();
const renderer = new GridRenderer($('grid'), {
  theme: currentTheme(),
  fontFamily: termFont(),
});
const encoder = new InputEncoder(() => model.modes);

let socket = null;
let decoder = new LineDecoder();
let sessionRows = [];
let activeRow = null;
let generation = 0;
let reconnectTimer = 0;
let reconnectDelay = 1000;
let listTimer = 0;
let loadingList = false;
let autoStarted = false;
let connecting = false;
let leaving = false;
let lastCols = 0;
let lastRows = 0;
let currentToken = '';
let viewportTop = 0;
let following = true;
let selection = null;
let selecting = false;
let selectAnchor = null;
let mouseHeld = null;
let lastMouseCell = null;
let focused = false;
let blinkOn = true;
let blinkTimer = 0;
let renderPending = false;
let fitPending = false;

globalThis.__grid = {model, renderer, socket: () => socket, state};
globalThis.__gridText = () => Array.from(
  {length: model.lineCount()},
  (_, i) => model.textOf(model.rowAt(i)),
).join('\n');

$('machine').textContent = node || location.hostname;

if (SessionDockCapabilities.config.terminal === false) {
  setStatus('此机器未启用终端传输');
} else {
  setup();
}

function currentTheme() {
  return document.documentElement.dataset.theme === 'light' ? 'light' : 'dark';
}

function termFont() {
  return getComputedStyle(document.documentElement).getPropertyValue('--terminal-font').trim()
    || 'UbuntuSansMono, Consolas, monospace';
}

function apiURL(path, query) {
  const url = new URL(prefix + path, base);
  if (query) {
    for (const [key, value] of Object.entries(query)) {
      if (value != null && value !== '') url.searchParams.set(key, String(value));
    }
  }
  return url;
}

function setStatus(message, error = false) {
  state.status = message;
  $('status').textContent = message;
  $('status').className = error ? 'error' : '';
}

function showTakeover(show) {
  $('takeover').hidden = !show;
}

function bindingOf(row) {
  if (!row) return {};
  if (row.record_id && row.launch_id && row.instance_id && !row.stale) {
    return {
      record_id: row.record_id,
      launch_id: row.launch_id,
      instance_id: row.instance_id,
    };
  }
  if (row.uid && row.instance_id) {
    return {uid: row.uid, instance_id: row.instance_id};
  }
  return {};
}

function rowByName(name) {
  return sessionRows.find(row => row.name === name) || null;
}

function hasOption(select, value) {
  for (const option of select.options) {
    if (option.value === value) return true;
  }
  return false;
}

function fillSelect(rows) {
  const select = $('session');
  const prev = select.value;
  select.replaceChildren();
  for (const row of rows) {
    const option = document.createElement('option');
    option.value = row.name;
    option.textContent = `${row.name} · ${row.cwd || ''}`;
    select.append(option);
  }
  if (wantedName && hasOption(select, wantedName) && !state.connected) select.value = wantedName;
  else if (hasOption(select, prev)) select.value = prev;
}

function maxTop() {
  return Math.max(0, model.lineCount() - model.rows);
}

function atBottom() {
  return viewportTop === maxTop();
}

function stickFollow() {
  if (following) viewportTop = maxTop();
  else viewportTop = Math.max(0, Math.min(viewportTop, maxTop()));
  following = atBottom();
  state.following = following;
}

function orderedSelection() {
  if (!selection) return null;
  const a = selection.start;
  const b = selection.end;
  if (!a || !b) return null;
  if (a.line === b.line && a.col === b.col) return null;
  if (a.line < b.line || (a.line === b.line && a.col <= b.col)) return selection;
  return {start: b, end: a};
}

function clearSelection() {
  if (!selection) return;
  selection = null;
  state.selection = null;
  scheduleRender();
}

function setSelection(next) {
  selection = next;
  state.selection = next;
}

function isWordChar(text) {
  if (!text) return false;
  const ch = text[0];
  if (ch === '_') return true;
  if (ch >= '0' && ch <= '9') return true;
  if ((ch >= 'A' && ch <= 'Z') || (ch >= 'a' && ch <= 'z')) return true;
  return ch.charCodeAt(0) > 127;
}

function selectWord(line, col) {
  const row = model.rowAt(line);
  if (!row) {
    setSelection({start: {line, col}, end: {line, col: col + 1}});
    return;
  }
  const cells = model.cellsOf(row);
  const texts = [];
  let used = 0;
  for (const cell of cells) {
    texts[used] = cell.text;
    for (let i = 1; i < cell.width; i++) texts[used + i] = '';
    used += cell.width;
  }
  if (!isWordChar(texts[col])) {
    setSelection({start: {line, col}, end: {line, col: col + 1}});
    return;
  }
  let start = col;
  let end = col + 1;
  while (start > 0 && isWordChar(texts[start - 1])) start--;
  while (end < texts.length && isWordChar(texts[end])) end++;
  setSelection({start: {line, col: start}, end: {line, col: end}});
}

function linkAt(cell) {
  const row = model.rowAt(cell.line);
  if (!row) return null;
  let used = 0;
  for (const item of model.cellsOf(row)) {
    if (cell.col >= used && cell.col < used + item.width) return item.link || null;
    used += item.width;
  }
  return null;
}

function cellFromEvent(event) {
  const rect = $('grid').getBoundingClientRect();
  return renderer.cellAt(event.clientX - rect.left, event.clientY - rect.top, viewportTop);
}

function syncTheme() {
  const theme = currentTheme();
  const family = termFont();
  if (renderer.themeName !== theme) renderer.setTheme(theme);
  if (family && renderer.fontFamily !== family) renderer.setFont(family, renderer.fontSize);
}

// 滚到回滚区顶部附近且服务端还有更早的历史时，按页拉取并插到最前面。
let loadingHistory = false;
async function maybeLoadHistory() {
  if (loadingHistory || readOnly || !state.connected || !activeRow) return;
  if (model.historyOlder <= 0 || viewportTop > 40) return;
  loadingHistory = true;
  try {
    const to = model.historyOlder;
    const from = Math.max(0, to - 500);
    const url = apiURL('term/grid/history', {
      name: activeRow.name, page: PAGE_ID, token: currentToken, from, to, ...bindingOf(activeRow),
    });
    const response = await fetch(url, {cache: 'no-store'});
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || `请求失败（${response.status}）`);
    const rows = Array.isArray(result.rows) ? result.rows : [];
    const added = model.prependHistory(rows);
    model.historyOlder = Math.max(0, from);
    // 视口跟着内容一起下移，用户看到的行不跳。
    viewportTop += added;
    following = false;
    stickFollow();
    scheduleRender();
  } catch (error) {
    model.historyOlder = 0; // 出错不再重试，避免刷屏
    setStatus(error.message || '读取历史失败', true);
  } finally {
    loadingHistory = false;
  }
}

function paint() {
  syncTheme();
  stickFollow();
  state.lastSeq = model.seq;
  renderer.render(model, {
    viewportTop,
    selection: orderedSelection(),
    focused,
    cursorBlinkOn: blinkOn,
  });
}

function scheduleRender() {
  if (renderPending) return;
  renderPending = true;
  requestAnimationFrame(() => {
    renderPending = false;
    paint();
  });
}

function startBlink() {
  stopBlink();
  blinkOn = true;
  blinkTimer = setInterval(() => {
    if (!focused) return;
    blinkOn = !blinkOn;
    scheduleRender();
  }, 530);
}

function stopBlink() {
  if (blinkTimer) {
    clearInterval(blinkTimer);
    blinkTimer = 0;
  }
}

function resetBlink() {
  blinkOn = true;
  if (focused) startBlink();
}

function applySize(sendIfOpen) {
  const box = $('term');
  const {cols, rows} = renderer.fit(box.clientWidth, box.clientHeight);
  state.cols = cols;
  state.rows = rows;
  $('size').textContent = `${cols}×${rows}`;
  if (readOnly) {
    scheduleRender();
    return {cols: model.cols, rows: model.rows};
  }
  if (cols === lastCols && rows === lastRows) return {cols, rows};
  lastCols = cols;
  lastRows = rows;
  model.resize(cols, rows);
  stickFollow();
  if (sendIfOpen && socket && socket.readyState === WebSocket.OPEN) {
    socket.send(encodeResize(cols, rows));
  }
  scheduleRender();
  return {cols, rows};
}

function scheduleFit() {
  if (fitPending) return;
  fitPending = true;
  requestAnimationFrame(() => {
    fitPending = false;
    applySize(true);
  });
}

function send(str) {
  if (readOnly) return;
  if (str == null || str === '') return;
  if (!socket || socket.readyState !== WebSocket.OPEN) return;
  socket.send(encoderUtf8.encode(str));
}

function sendInput(str) {
  clearSelection();
  send(str);
}

function mouseSeq(kind, button, cell, event, wheelDelta) {
  if (readOnly) return null;
  const col = cell.col;
  const row = cell.line - viewportTop;
  return encoder.mouse(kind, button, col, row, {
    shift: !!event.shiftKey,
    alt: !!event.altKey,
    ctrl: !!event.ctrlKey,
  }, wheelDelta);
}

function dropSocket() {
  generation += 1;
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = 0;
  }
  const ws = socket;
  socket = null;
  state.connected = false;
  if (ws) {
    try { ws.close(); } catch {}
  }
}

function startListRefresh() {
  if (listTimer) return;
  listTimer = setInterval(() => {
    if (!state.connected) loadList();
  }, 5000);
}

function stopListRefresh() {
  if (!listTimer) return;
  clearInterval(listTimer);
  listTimer = 0;
}

async function loadList() {
  if (loadingList) return;
  loadingList = true;
  try {
    const response = await fetch(apiURL('term/list'), {cache: 'no-store'});
    if (!(response.headers.get('Content-Type') || '').includes('application/json')) {
      throw new Error('无法读取响应，请确认登录状态后刷新');
    }
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || `请求失败（${response.status}）`);
    sessionRows = Array.isArray(result.sessions) ? result.sessions : [];
    fillSelect(sessionRows);
    if (!state.connected && !state.status) setStatus(sessionRows.length ? '' : '没有会话');
    if (wantedName && !autoStarted && !state.connected && rowByName(wantedName)) {
      autoStarted = true;
      $('session').value = wantedName;
      connect();
    }
  } catch (error) {
    if (!state.connected) setStatus(error.message, true);
  } finally {
    loadingList = false;
  }
}

async function claim(row, force = false) {
  const body = {name: row.name, page: PAGE_ID, ...bindingOf(row)};
  if (force) body.force = true;
  const response = await fetch(apiURL('term/claim'), {
    method: 'POST',
    headers: {'Content-Type': 'application/json'},
    body: JSON.stringify(body),
  });
  let result = {};
  try { result = await response.json(); } catch {}
  return {response, result};
}

function openSocket(row, token, cols, rows) {
  decoder = new LineDecoder();
  const gen = generation;
  const url = apiURL('term/attach', {
    name: row.name,
    page: PAGE_ID,
    token,
    connection: crypto.randomUUID(),
    cols,
    rows,
    mode: 'grid',
    ...bindingOf(row),
  });
  url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const ws = new WebSocket(url.href);
  ws.binaryType = 'arraybuffer';
  socket = ws;

  ws.addEventListener('open', () => {
    if (gen !== generation || socket !== ws) return;
    state.connected = true;
    reconnectDelay = 1000;
    stopListRefresh();
    setStatus('已连接');
    applySize(true);
  });

  ws.addEventListener('message', event => {
    if (gen !== generation || socket !== ws) return;
    if (typeof event.data === 'string') {
      let message;
      try { message = JSON.parse(event.data); } catch { return; }
      if (message && message.t === 'revoked') {
        leaving = true;
        dropSocket();
        startListRefresh();
        setStatus(`控制权已被 ${message.by || ''} 抢占`);
        return;
      }
      return;
    }
    const messages = decoder.push(event.data);
    if (!messages.length) return;
    for (const msg of messages) {
      model.apply(msg);
      if (msg && (msg.t === 'diff' || msg.t === 'snapshot') && msg.title != null) {
        document.title = `${msg.title} · 网格终端 · SessionDock`;
      }
    }
    resetBlink();
    state.lastSeq = model.seq;
    stickFollow();
    scheduleRender();
  });

  ws.addEventListener('close', event => {
    if (gen !== generation) return;
    if (socket === ws) socket = null;
    state.connected = false;
    if (leaving) return;
    if (event.code === 1000 && event.reason === 'host exited') {
      setStatus('终端进程已退出');
      startListRefresh();
      return;
    }
    if (event.code === 4001 || event.code === 4002) {
      setStatus(event.reason || `连接已关闭（${event.code}）`, true);
      startListRefresh();
      return;
    }
    const delay = reconnectDelay;
    reconnectDelay = Math.min(reconnectDelay * 2, 30000);
    setStatus(`连接断开${event.reason ? '：' + event.reason : ''}，${delay / 1000}s 后重试`);
    reconnectTimer = setTimeout(() => {
      reconnectTimer = 0;
      if (leaving) return;
      connect(false, true);
    }, delay);
  });
}

function openRecordSocket(id) {
  decoder = new LineDecoder();
  const gen = generation;
  const url = apiURL('term/records/attach', {id, mode: 'grid'});
  url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const ws = new WebSocket(url.href);
  ws.binaryType = 'arraybuffer';
  socket = ws;
  ws.addEventListener('open', () => {
    if (gen !== generation || socket !== ws) return;
    state.connected = true;
    reconnectDelay = 1000;
    setStatus('正在回放…');
  });
  ws.addEventListener('message', event => {
    if (gen !== generation || socket !== ws) return;
    if (typeof event.data === 'string') {
      let message;
      try { message = JSON.parse(event.data); } catch { return; }
      if (!message) return;
      if (message.t === 'record') {
        state.live = !!message.live;
        state.ended = !message.live;
        setStatus(message.live ? '录制进行中 · 实时跟随' : '录制已结束');
        $('size').textContent = `${message.cols}×${message.rows}`;
      } else if (message.t === 'gap') {
        setStatus(state.status + '（录制有缺口，已从下一个快照继续）');
      } else if (message.t === 'exit') {
        const exit = message.exit || {};
        state.live = false;
        state.ended = true;
        setStatus(`录制已结束 · 退出码 ${exit.code}${exit.output_complete === false ? ' · 输出不完整' : ''}`);
      } else if (message.t === 'end') {
        state.ended = true;
        setStatus(state.status + ' · 回放完成');
      }
      return;
    }
    const messages = decoder.push(event.data);
    if (!messages.length) return;
    for (const msg of messages) {
      model.apply(msg);
      if (msg && msg.t === 'snapshot') $('size').textContent = `${model.cols}×${model.rows}`;
    }
    state.lastSeq = model.seq;
    stickFollow();
    scheduleRender();
  });
  ws.addEventListener('close', event => {
    if (gen !== generation) return;
    if (socket === ws) socket = null;
    state.connected = false;
    if (leaving || event.code === 1000 || !state.live) return;
    const delay = reconnectDelay;
    reconnectDelay = Math.min(reconnectDelay * 2, 30000);
    setStatus(`连接断开${event.reason ? '：' + event.reason : ''}，${delay / 1000}s 后重试`);
    reconnectTimer = setTimeout(() => {
      reconnectTimer = 0;
      if (!leaving) openRecordSocket(id);
    }, delay);
  });
}

async function connect(force = false, fromReconnect = false) {
  if (connecting) return;
  const row = fromReconnect && activeRow ? activeRow : rowByName($('session').value);
  if (!row) {
    setStatus('请选择会话', true);
    return;
  }
  connecting = true;
  leaving = false;
  showTakeover(false);
  dropSocket();
  const gen = generation;
  stopListRefresh();
  activeRow = row;
  state.name = row.name;
  setStatus('正在连接…');
  const {cols, rows} = applySize(false);
  try {
    const {result} = await claim(row, force);
    if (leaving || gen !== generation) return;
    if (result.conflict) {
      const label = result.owner && result.owner.label ? result.owner.label : '另一页面';
      showTakeover(true);
      setStatus(`该终端正由 ${label} 控制`);
      startListRefresh();
      return;
    }
    if (result.error || !result.token) {
      setStatus(result.error || '无法取得终端控制权', true);
      startListRefresh();
      return;
    }
    currentToken = result.token;
    openSocket(row, result.token, cols, rows);
  } catch (error) {
    setStatus(error.message || '连接失败', true);
    startListRefresh();
  } finally {
    connecting = false;
  }
}

function setup() {
  const keys = $('keys');
  keys.addEventListener('keydown', event => {
    const range = orderedSelection();
    if ((event.ctrlKey || event.metaKey) && event.key === 'c' && range) {
      const text = model.selectionText(range.start, range.end);
      if (text) {
        event.preventDefault();
        event.stopImmediatePropagation();
        navigator.clipboard.writeText(text);
        clearSelection();
      }
    }
  });
  new KeyCapture(keys, {
    encoder,
    onBytes: sendInput,
    onPaste: text => sendInput(encoder.paste(text)),
  });

  keys.addEventListener('focus', () => {
    focused = true;
    resetBlink();
    const seq = encoder.focus(true);
    if (seq != null) send(seq);
    scheduleRender();
  });
  keys.addEventListener('blur', () => {
    focused = false;
    stopBlink();
    blinkOn = true;
    const seq = encoder.focus(false);
    if (seq != null) send(seq);
    scheduleRender();
  });

  $('term').addEventListener('click', () => keys.focus());
  $('term').addEventListener('wheel', event => {
    event.preventDefault();
    const modes = model.modes;
    if (!readOnly && modes.alt && modes.mouse === 'none') {
      send(encoder.wheelAsArrows(event.deltaY));
      return;
    }
    if (!readOnly && modes.mouse !== 'none') {
      const cell = cellFromEvent(event);
      send(mouseSeq('wheel', 0, cell, event, event.deltaY));
      return;
    }
    following = false;
    viewportTop += event.deltaY < 0 ? -3 : 3;
    stickFollow();
    scheduleRender();
    if (event.deltaY < 0) maybeLoadHistory();
  }, {passive: false});

  $('grid').addEventListener('mousedown', event => {
    keys.focus();
    const cell = cellFromEvent(event);
    // Ctrl/⌘ + 左键：打开该格子上的 OSC 8 超链接（新标签，noopener）。
    if (event.button === 0 && (event.ctrlKey || event.metaKey)) {
      const link = linkAt(cell);
      if (link) {
        event.preventDefault();
        window.open(link, '_blank', 'noopener');
        return;
      }
    }
    if (!readOnly && model.modes.mouse !== 'none' && !event.shiftKey) {
      event.preventDefault();
      send(mouseSeq('down', event.button, cell, event));
      mouseHeld = {button: event.button};
      lastMouseCell = cell;
      return;
    }
    if (event.button !== 0) return;
    event.preventDefault();
    selectAnchor = {line: cell.line, col: cell.col};
    if (event.detail >= 2) {
      selecting = false;
      selectWord(cell.line, cell.col);
      scheduleRender();
      return;
    }
    selecting = true;
    setSelection({start: selectAnchor, end: selectAnchor});
    scheduleRender();
  });

  addEventListener('mousemove', event => {
    const cell = cellFromEvent(event);
    if (!readOnly && (mouseHeld || model.modes.mouse === 'any_motion')) {
      const button = mouseHeld ? mouseHeld.button : 0;
      if (!lastMouseCell || lastMouseCell.col !== cell.col || lastMouseCell.line !== cell.line) {
        send(mouseSeq('move', button, cell, event));
        lastMouseCell = cell;
      }
    }
    if (!selecting || !selectAnchor) return;
    const a = selectAnchor;
    const b = {line: cell.line, col: cell.col};
    const backward = b.line < a.line || (b.line === a.line && b.col < a.col);
    setSelection(backward
      ? {start: b, end: {line: a.line, col: a.col + 1}}
      : {start: a, end: {line: b.line, col: b.col + 1}});
    scheduleRender();
  });

  addEventListener('mouseup', event => {
    if (mouseHeld) {
      send(mouseSeq('up', mouseHeld.button, cellFromEvent(event), event));
      mouseHeld = null;
      lastMouseCell = null;
    }
    if (selecting) {
      selecting = false;
      if (selection && selection.start.line === selection.end.line
          && selection.start.col === selection.end.col) {
        clearSelection();
      }
    }
  });

  $('connect').addEventListener('click', () => connect(false));
  $('takeover').addEventListener('click', () => connect(true));
  $('copy').addEventListener('click', () => {
    const range = orderedSelection();
    if (!range) return;
    const text = model.selectionText(range.start, range.end);
    if (text) navigator.clipboard.writeText(text);
  });
  $('paste').addEventListener('click', async () => {
    try {
      const text = await navigator.clipboard.readText();
      sendInput(encoder.paste(text));
    } catch (error) {
      setStatus(error.message || '粘贴失败', true);
    }
  });
  $('bottom').addEventListener('click', () => {
    following = true;
    viewportTop = maxTop();
    state.following = true;
    scheduleRender();
  });

  addEventListener('resize', scheduleFit);
  addEventListener('pagehide', () => {
    leaving = true;
    dropSocket();
  });
  new MutationObserver(() => scheduleRender()).observe(
    document.documentElement,
    {attributes: true, attributeFilter: ['data-theme']},
  );

  applySize(false);
  if (readOnly) {
    // 录制回放：隐藏会话选择与抢占，键盘/鼠标只用于滚动与选区。
    $('session').hidden = true;
    $('connect').hidden = true;
    $('paste').hidden = true;
    state.name = wantedRecord;
    setStatus('正在连接…');
    openRecordSocket(wantedRecord);
    return;
  }
  startListRefresh();
  loadList();
}
