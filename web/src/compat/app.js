'use strict';

const SOURCES = Object.freeze(Object.fromEntries(
  [...Object.values(SESSIONDOCK_CLIS), {source: 'shell', name: 'SSH', icon: 'i-terminal', color: 'var(--muted)'}].map(cli => [cli.source, {
    name: cli.name, icon: cli.icon, color: cli.color,
  }])));

// 页面偏好落 localStorage；输入内容由服务端会话草稿保存。
// 读取缺失时回退到旧前缀下的同名键并一次性搬到新键；写只写新键。
const store = {
  get(k, d) {
    try {
      const v = SessionDockCapabilities.stored(k, STORAGE_PREFIX);
      return v === null ? d : JSON.parse(v);
    } catch { return d; }
  },
  set: (k, v) => localStorage.setItem(STORAGE_PREFIX + k, JSON.stringify(v)),
};

const shellBridge = {
  get: (key, fallback) => store.get(key, fallback),
  set: (key, value) => store.set(key, value),
  openNewSession: () => openNewSessionDialog(), openSettings: () => openSettings(),
  openTrash: () => openTrash(),
  openTransfers: () => openTransferTasks(),
  selectView: value => { S.view = value; store.set('view', S.view); renderView(); renderSide(); },
  toggleNest: () => { S.nest = !S.nest; store.set('nest', S.nest); renderView(); renderSide(); },
  layoutSessionHead: () => layoutSessionHead(),
  fitTerminal: () => { if (typeof fitTerm === 'function' && T?.term) fitTerm(); },
  fitViewportTerminal: () => { if (typeof fitTerm === 'function') fitTerm(); },
  closeTerminal: () => { if (typeof T !== 'undefined' && !$('#termpane').classList.contains('hidden')) closeTermPane(true); },
  selected: () => !!S.sel,
  freezeOverlay: () => syncSessionFreezeOverlay(), stopNotice: () => syncSessionStopNotice(),
  layoutTerminal: () => { if (typeof layoutTermPane !== 'function') return false; layoutTermPane(); return true; },
  scrollTerminal: () => { try { currentTermViewObject()?.term?.scrollToBottom(); } catch { /* no view yet */ } },
  positionTerminal: () => { if (typeof positionTermViewport === 'function') positionTermViewport(currentTermViewObject()); },
  cancelLongPress: () => cancelLongPress(), closeItemMenu: () => closeItemMenu(),
  resetItemClick: () => { suppressItemClick = false; },
  refreshTerminalScale: persist => { if (typeof refreshTerminalScale === 'function') refreshTerminalScale(persist); },
  updateScale: scale => SessionDockSettings.update({scale}),
};
SessionDockShell.configure(shellBridge);

let settingsMounted = false;

const FONT_CHOICES = SessionDockTypography.choices;
const themeMedia = matchMedia('(prefers-color-scheme: dark)');

function applyTheme(choice = store.get('theme', 'system'), persist = false) {
  if (!['system', 'light', 'dark'].includes(choice)) choice = 'system';
  if (persist) store.set('theme', choice);
  document.documentElement.dataset.theme = choice === 'system'
    ? (themeMedia.matches ? 'dark' : 'light') : choice;
  if (settingsMounted) SessionDockSettings.update({theme: choice});
  if (typeof refreshTerminalPreferences === 'function') refreshTerminalPreferences(true);
}

function applyFont(choice = store.get('font', 'ubuntu'), persist = false) {
  if (!FONT_CHOICES[choice]) choice = 'ubuntu';
  if (persist) store.set('font', choice);
  document.documentElement.style.setProperty('--terminal-font', FONT_CHOICES[choice]);
  if (settingsMounted) SessionDockSettings.update({font: choice});
  if (typeof refreshTerminalPreferences === 'function') refreshTerminalPreferences(false);
}

// Named compatibility callbacks; implementations and gesture lifetime belong to the shell.
function normalizedInterfaceScale(value) { return SessionDockShell.normalizedInterfaceScale(value); }
function interfaceScale() { return SessionDockShell.interfaceScale(); }
function applyInterfaceScale(value = interfaceScale(), persist = false) { SessionDockShell.applyInterfaceScale(value, persist); }
function showScaleIndicator(value, active = false) { SessionDockShell.showScaleIndicator(value, active); }
const SessionDockGestures = SessionDockShell.gestures;
applyInterfaceScale();

themeMedia.addEventListener('change', () => {
  if (store.get('theme', 'system') === 'system') applyTheme('system');
});
applyTheme();
applyFont();

const S = {
  sessions: [],
  view: store.get('view', 'tree'),
  nest: store.get('nest', false),  // 左栏分层：由会话发起的会话缩进在发起者下；子代理行两种模式都挂
  nestClosed: new Set(store.get('nestClosed', [])),  // 手动收起的发起者 uid（平铺收起子代理行，分层收起整棵子树）
  off: new Set(store.get('off', [])),
  closed: new Set(store.get('closed', [])),
  searchClosed: new Set(),
  searchNestClosed: new Set(),
  sel: null,
  term: '',           // 当前要高亮的词 (= 搜索框内容)
  opts: Object.assign({ case: false, word: false, regex: false, mode: 'all' }, store.get('opts', {})),
  cur: -1,            // 匹配跳转游标
  autoOpen: 0,        // 本次渲染已自动展开的命中消息数
  markCapped: false,  // 高亮是否因数量上限被截断
  results: null,      // 全文搜索结果, null 表示未处于搜索态
  agent: null,        // 当前查看的子代理 id；null = 主会话
  syncGap: 350,       // 当前会话的同步间隔, 随有无新内容自适应
  live: new Set(),    // 仍在运行的会话 uid
  liveTmux: new Set(),// 其中运行在 tmux 里的会话 uid
  liveWorking: new Set(),// 有明确归属的后台命令进程仍在运行
  liveStarted: new Map(), // uid → 当前 CLI 主进程启动时间（Unix 秒）
  activeOnly: SessionDockCapabilities.allows('live') && store.get('activeOnly', false), // 未探测时不按空集合筛选
  compactTurns: store.get('compactTurns', true), // 已完成回合只保留过程合集与最终结论
  unread: new Map(store.get('unread', [])),   // uid → {count, tmux}; 只计代理产生的新内容
  cursors: new Map(), // 主会话/子代理 EOF 游标；用于后台会话的精确未读增量
  starBusy: new Set(), // 正在持久化星标的会话，避免多个网页请求在服务端乱序
  picking: false,     // 左栏多选模式；刻意不持久化，刷新后回到普通浏览
  nestAttach: '',     // 附属点选：等待点击父会话的子会话 uid（多选时为第一条）；不持久化
  nestAttachUids: [], // 附属点选的全部子会话 uid（单条或多选）
  sig: null,          // 列表对应的磁盘签名
  lastSync: 0,
};

const $ = s => document.querySelector(s);
const MOBILE = matchMedia('(max-width: 720px)');
// 顶栏和会话头按三级宽度排版：窄屏 ≤720，中屏 721–1199，宽屏 ≥1200。断点与 style.css 一致。
const MEDIUM = matchMedia('(max-width: 1199px)');
function layoutTier() { return MOBILE.matches ? 'narrow' : MEDIUM.matches ? 'medium' : 'wide'; }
// 页面既可挂在站点根目录，也可由反代放到 /sessiondock/ 之类的子路径。
const APP_BASE = new URL('.', location.href);
// 深链：?sid=<source>:<sid> 或 ?sid=<sid>，打开指定会话（labdesk 的会话台账用它跳过来）。
// 用 CLI 原生会话号而不是 uid —— uid 是会话文件路径的散列，换目录就变。
const DEEP_SID = (new URLSearchParams(location.search).get('sid') || '').trim().slice(0, 128);
const deepNode = () => new URLSearchParams(location.search).get('node') || '';
const appUrl = path => {
  const url = new URL(String(path).replace(/^\//, ''), APP_BASE);
  if (HUB_MODE && !url.searchParams.has('nodes') && /\/api\/(search|trash|trash\/purge)$/.test(url.pathname)) {
    url.searchParams.set('nodes', selectedNodeIds().join(','));
  }
  if (HUB_MODE && /\/api\/term\/(complete-dir|models)$/.test(url.pathname)
      && !url.searchParams.get('node')) url.searchParams.set('node', newNodeId());
  return url.toString();
};
const BUILD_ID = document.querySelector('meta[name="sessiondock-build"]')?.content || '';
// One ephemeral page identity joins HTTP, SSE, terminal and final DOM receipts.
// It intentionally is not persisted: duplicated/restored tabs must remain distinct.
const AUDIT_PAGE_ID = globalThis.crypto?.randomUUID?.()
  || [...globalThis.crypto.getRandomValues(new Uint8Array(16))]
    .map(value => value.toString(16).padStart(2, '0')).join('');
window.__sessiondockPageId = AUDIT_PAGE_ID;

let browserAuditQueue = [];
let browserAuditTimer = 0;
let browserAuditDue = 0;
let browserAuditRetryAt = 0;
let browserAuditSending = false;
let browserAuditFailCount = 0;
const AUDIT_BATCH_BYTES = 48 * 1024;
const AUDIT_BATCH_COUNT = 100;
const AUDIT_FLUSH_MS = 5000;
const AUDIT_CONTENT_LIMIT = 32 * 1024;

// 按 UTF-8 字节算：sendBeacon 的 64 KB 配额是字节数，中文一个字占 3 字节
const auditEncoder = new TextEncoder();
function auditEventBytes(event) {
  try { return auditEncoder.encode(JSON.stringify(event)).length; } catch { return AUDIT_BATCH_BYTES + 1; }
}

function spliceAuditBatch(queue, limit = AUDIT_BATCH_BYTES, count = AUDIT_BATCH_COUNT) {
  if (!queue.length) return [];
  if (auditEventBytes(queue[0]) > limit) return queue.splice(0, 1);
  const batch = [];
  let bytes = 0;
  while (queue.length && batch.length < count) {
    const size = auditEventBytes(queue[0]);
    if (bytes + size > limit) break;
    batch.push(queue.shift());
    bytes += size;
  }
  return batch;
}

function capAuditQueue() {
  if (browserAuditQueue.length > 500) browserAuditQueue.splice(0, browserAuditQueue.length - 500);
}

// Ordinary diagnostics share a batch across several polling cycles. Full
// batches and failures flush sooner; another info event never postpones a flush.
function scheduleBrowserAudit(delay = AUDIT_FLUSH_MS) {
  delay = Math.max(delay, browserAuditRetryAt - performance.now());
  const due = performance.now() + delay;
  if (browserAuditTimer && browserAuditDue <= due) return;
  clearTimeout(browserAuditTimer);
  browserAuditDue = due;
  browserAuditTimer = setTimeout(flushBrowserAudit, delay);
}

function browserAuditEvent(event, data = {}, content = null, fields = {}) {
  if (!SessionDockCapabilities.allows('audit')) return;
  try {
    // Rust stores metadata only; sending message/DOM/terminal bodies here
    // wastes bandwidth and prematurely fills the byte-limited audit batches.
    let auditContent = SessionDockCapabilities.config.backend === 'rust' ? null : content;
    let auditData = data;
    try {
      if (auditContent != null) {
        const serialized = JSON.stringify(auditContent);
        if (serialized.length > AUDIT_CONTENT_LIMIT) {
          auditContent = {truncated: true, bytes: serialized.length,
            head: serialized.slice(0, 8 * 1024)};
          auditData = {...data, content_truncated: true};
        }
      }
    } catch {
      auditContent = null;
    }
    browserAuditQueue.push({
      event, ts: new Date().toISOString(), uid: fields.uid ?? S.sel ?? '',
      trace_id: fields.traceId || '', request_id: fields.requestId || '',
      connection_id: fields.connectionId || '', severity: fields.severity || 'info',
      data: auditData, content: auditContent,
    });
    capAuditQueue();
    scheduleBrowserAudit(browserAuditQueue.length >= AUDIT_BATCH_COUNT ? 0
      : fields.severity === 'error' ? 100 : AUDIT_FLUSH_MS);
  } catch { /* diagnostics never change UI behavior */ }
}

function auditPayload(events) {
  return JSON.stringify({
    page_id: AUDIT_PAGE_ID, uid: S.sel || '', _build: BUILD_ID, events,
  });
}

async function flushBrowserAudit() {
  if (!SessionDockCapabilities.allows('audit')) return;
  clearTimeout(browserAuditTimer);
  browserAuditTimer = 0;
  if (SessionDockNetwork.paused) return;
  if (browserAuditSending || !browserAuditQueue.length) return;
  if (performance.now() < browserAuditRetryAt) { scheduleBrowserAudit(); return; }
  const events = spliceAuditBatch(browserAuditQueue);
  if (!events.length) return;
  browserAuditSending = true;
  try {
    const response = await fetch(appUrl('api/audit/browser'), {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json', 'X-SessionDock-Page': AUDIT_PAGE_ID,
        'X-SessionDock-Build': BUILD_ID,
      },
      body: auditPayload(events),
    });
    // 必须把响应体读完：没读的 fetch 响应在渲染进程里各占着一条 2 MiB 共享内存
    // 数据管道（一个 fd），直到 GC 才释放。这里每秒一条，渲染进程 1024 个 fd 的
    // 上限十几分钟就满，之后 GPU 命令缓冲区拿不到共享内存，整页在原生代码里卡死。
    await response.arrayBuffer().catch(() => {});
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    browserAuditFailCount = 0;
    browserAuditRetryAt = 0;
  } catch (error) {
    browserAuditRetryAt = performance.now() + AUDIT_FLUSH_MS;
    // 断网期间只是攒着，不算失败次数；连续三次在线失败才认定这一批本身有问题
    if (navigator.onLine) browserAuditFailCount += 1;
    if (browserAuditFailCount >= 3) {
      browserAuditFailCount = 0;
      const bytes = events.reduce((n, ev) => n + auditEventBytes(ev), 0);
      browserAuditQueue.push({
        event: 'audit.dropped', ts: new Date().toISOString(), uid: S.sel ?? '',
        trace_id: '', request_id: '', connection_id: '', severity: 'info',
        data: {events: events.length, bytes,
          reason: String(error?.message || error || '').slice(0, 200)},
        content: null,
      });
      capAuditQueue();
    } else {
      browserAuditQueue.unshift(...events);
      if (browserAuditQueue.length > 500) browserAuditQueue.length = 500;
    }
  } finally {
    browserAuditSending = false;
    if (browserAuditQueue.length && !browserAuditTimer) {
      scheduleBrowserAudit(browserAuditQueue.length >= AUDIT_BATCH_COUNT ? 0 : AUDIT_FLUSH_MS);
    }
  }
}

// 卸载时整个页面只有 64 KB 的 beacon 配额（和 keepalive 同一个池），超出的包
// sendBeacon 直接返回 false。所以先把各事件的 content 丢掉保住事件本身，只给
// page.hidden 那条快照留 content；分两包发，合计不超过 60 KB。发不出去的留在
// 队列里，页面若只是进了 bfcache 还能补发。
function flushBrowserAuditBeacon() {
  if (SessionDockNetwork.paused) return;
  if (!SessionDockCapabilities.allows('audit')) return;
  clearTimeout(browserAuditTimer);
  browserAuditTimer = 0;
  if (!navigator.sendBeacon || !browserAuditQueue.length) return;
  const slim = browserAuditQueue.map(ev => ev.event === 'page.hidden' || ev.content == null ? ev
    : {...ev, content: null, data: {...ev.data, content_dropped: true}});
  browserAuditQueue = [];
  for (const budget of [40 * 1024, 20 * 1024]) {
    const events = spliceAuditBatch(slim, budget, 100);
    if (!events.length) break;
    const ok = navigator.sendBeacon(appUrl('api/audit/browser'),
      new Blob([auditPayload(events)], {type: 'application/json'}));
    if (!ok) { slim.unshift(...events); break; }
  }
  if (slim.length) {
    browserAuditQueue.unshift(...slim);
    capAuditQueue();
  }
}

// ---- 主线程长帧归因 ----
// 页面偶发整体卡死，事后只能从审计日志看到轮询停摆，看不到是谁在跑。卡死前
// 通常先有几段逐步变长的长任务；long-animation-frame 由浏览器给出每段里跑了
// 哪个函数、在哪个文件的哪个位置、由谁触发、耗时和强制布局占比。这里对 ≥1 s
// 的帧各记一条 main_thread.long_frame，用 sendBeacon 直接交给浏览器进程发出，
// 随后主线程再卡死也不会丢。每页最多记 60 条，免得一个坏循环刷爆日志。
const LONG_FRAME_MIN_MS = 1000;
const LONG_FRAME_MAX_EVENTS = 60;
let longFrameCount = 0;
function longFrameEvent(entry) {
  const base = APP_BASE.href;
  const scripts = [...(entry.scripts || [])].slice(0, 8).map(script => ({
    invoker: String(script.invoker || '').slice(0, 200), invoker_type: script.invokerType || '',
    function: String(script.sourceFunctionName || '').slice(0, 120),
    url: String(script.sourceURL || '').replace(base, ''), char: script.sourceCharPosition ?? null,
    duration_ms: Math.round(script.duration || 0),
    forced_layout_ms: Math.round(script.forcedStyleAndLayoutDuration || 0),
    pause_ms: Math.round(script.pauseDuration || 0),
  }));
  const end = entry.startTime + entry.duration;
  const memory = performance.memory;
  const term = typeof T === 'undefined' ? null
    : {name: T.name, views: T.views?.size ?? 0, connected: T.ws?.readyState ?? null};
  return {
    event: 'main_thread.long_frame',
    ts: new Date(performance.timeOrigin + entry.startTime).toISOString(),
    uid: S.sel ?? '', trace_id: '', request_id: '', connection_id: '', severity: 'warning',
    data: {
      duration_ms: Math.round(entry.duration), blocking_ms: Math.round(entry.blockingDuration || 0),
      render_ms: entry.renderStart ? Math.round(end - entry.renderStart) : 0,
      style_layout_ms: entry.styleAndLayoutStart ? Math.round(end - entry.styleAndLayoutStart) : 0,
      scripts,
      heap_mb: memory ? [memory.usedJSHeapSize, memory.totalJSHeapSize, memory.jsHeapSizeLimit]
        .map(bytes => Math.round(bytes / 1048576)) : null,
      dom_nodes: document.getElementsByTagName('*').length,
      selected: S.sel, agent: S.agent, visibility: document.visibilityState,
      audit_queue: browserAuditQueue.length, terminal: term,
    },
    content: null,
  };
}
function observeLongFrames() {
  if (!SessionDockCapabilities.allows('audit')) return;
  if (!globalThis.PerformanceObserver?.supportedEntryTypes?.includes('long-animation-frame')) return;
  const observer = new PerformanceObserver(list => {
    if (SessionDockNetwork.paused) return;
    for (const entry of list.getEntries()) {
      if (entry.duration < LONG_FRAME_MIN_MS || longFrameCount >= LONG_FRAME_MAX_EVENTS) continue;
      longFrameCount += 1;
      try {
        const event = longFrameEvent(entry);
        const sent = navigator.sendBeacon?.(appUrl('api/audit/browser'),
          new Blob([auditPayload([event])], {type: 'application/json'}));
        if (!sent) browserAuditEvent(event.event, event.data, null, {severity: event.severity});
      } catch { /* 诊断不影响页面 */ }
    }
  });
  observer.observe({type: 'long-animation-frame', buffered: true});
}
observeLongFrames();

const nativeAlert = window.alert.bind(window);
const nativeConfirm = window.confirm.bind(window);
window.alert = message => {
  const text = String(message ?? '').slice(0, 500);
  browserAuditEvent('dialog.shown', {kind: 'alert', text});
  try { return nativeAlert(message); }
  finally { browserAuditEvent('dialog.closed', {kind: 'alert', result: null}); }
};
window.confirm = message => {
  const text = String(message ?? '').slice(0, 500);
  browserAuditEvent('dialog.shown', {kind: 'confirm', text});
  let result = null;
  try { return (result = nativeConfirm(message)); }
  finally { browserAuditEvent('dialog.closed', {kind: 'confirm', result}); }
};

function browserStateSnapshot(reason = '') {
  const box = $('#msgs');
  const nodes = box ? [...box.querySelectorAll('.msg, #activity')].slice(-40) : [];
  const entry = S.sel ? cache.get(viewKey(S.sel, S.agent)) : null;
  const termState = typeof T === 'undefined' ? null : {
    name: T.name, uid: T.uid, mode: T.mode,
    visible: !$('#termpane')?.classList.contains('hidden'),
    connected: T.ws?.readyState ?? null,
    frozen: (T.list || []).find(row => row.uid === S.sel)?.frozen ?? null,
  };
  return {
    data: {
      reason, selected: S.sel, agent: S.agent, mobile: MOBILE.matches,
      mobile_detail: document.body.classList.contains('mobile-detail'),
      visibility: document.visibilityState, online: navigator.onLine,
      viewport: {width: innerWidth, height: innerHeight,
        visual_width: window.visualViewport?.width,
        visual_height: window.visualViewport?.height},
      cache: entry ? {messages: entry.msgs?.length || 0, end: entry.end,
        anchor: entry.anchor, activity: entry.activity?.state || ''} : null,
      dom_messages: nodes.length, terminal: termState,
      header: consoleButtonState(),
    },
    content: {
      composer: $('#cinput')?.value || '',
      messages: nodes.map(node => ({
        id: node.id || '', role: node.dataset?.role || '',
        call_id: node.dataset?.callId || '', classes: node.className,
        text: (node.textContent || '').slice(0, 4000),
      })),
    },
  };
}

let browserSnapshotTimer = 0;
let lastBrowserSnapshot = '';
function scheduleBrowserSnapshot(reason = 'render') {
  if (!SessionDockCapabilities.allows('audit')) return;
  clearTimeout(browserSnapshotTimer);
  browserSnapshotTimer = setTimeout(() => {
    try {
      const snapshot = browserStateSnapshot(reason);
      const signature = JSON.stringify(snapshot);
      if (signature === lastBrowserSnapshot) return;
      lastBrowserSnapshot = signature;
      browserAuditEvent('dom.snapshot', snapshot.data, snapshot.content);
    } catch { /* page can be between detail teardown and rebuild */ }
  }, 100);
}

// ---- 会话标题栏 / 控制台按钮的存在性审计 ----
// 控制台按钮"偶尔消失"一直没抓到现场：快照只看消息，不看标题栏。这里独立记录
// 按钮的三层状态——在不在 DOM、几何上有没有被裁掉/盖住（elementFromPoint）、
// #right 有没有被滚走——任何一层不成立就记一条 console.button.missing，附上
// 标题栏 HTML 和布局数据；恢复时记 console.button.restored。
// 常规状态变化（文案、接管态、灰态、位置）按签名去重记 console.button.state。
function consoleButtonState() {
  const button = $('#a-term');
  const right = $('#right');
  const detail = $('#detail');
  const head = detail?.querySelector(':scope > .dhead');
  const shown = !!right && right.offsetWidth > 0;   // 手机列表页 #right 整个 display:none
  const state = {
    present: !!button, head: !!head, shown,
    detail_children: detail ? [...detail.children].map(
      node => node.id || node.className.split(' ')[0] || node.tagName.toLowerCase()).slice(0, 8) : null,
    right_scroll: right ? [right.scrollLeft, right.scrollTop] : null,
    right_overflow: right ? [right.scrollWidth - right.clientWidth, right.scrollHeight - right.clientHeight] : null,
    tier: layoutTier(), mobile_detail: document.body.classList.contains('mobile-detail'),
  };
  if (button) {
    const style = getComputedStyle(button);
    state.label = button.ariaLabel || '';
    state.on = button.classList.contains('on');
    state.unavailable = button.dataset.unavailable === 'true';
    state.hidden = button.hidden || style.display === 'none' || style.visibility !== 'visible'
      || parseFloat(style.opacity) === 0;
  }
  if (button && shown) {
    const r = button.getBoundingClientRect(), rr = right.getBoundingClientRect();
    const hit = r.width && r.height && document.visibilityState === 'visible'
      ? document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2) : undefined;
    state.rect = [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)];
    state.in_right = r.width > 0 && r.right <= rr.right + .5 && r.left >= rr.left - .5
      && r.top >= rr.top - .5 && r.bottom <= rr.bottom + .5;
    // 页面不可见时 elementFromPoint 一律 null，不算被遮
    state.hit = hit === undefined ? null : !!hit && button.contains(hit);
    // SVG 元素的 className 是 SVGAnimatedString，只能从 classList 取
    state.hit_target = hit ? (hit.id ? '#' + hit.id : hit.classList?.[0] ? '.' + hit.classList[0]
      : hit.tagName.toLowerCase()) : null;
  }
  // 不在 DOM 一定算丢；在 DOM 但页面可见时被隐藏/裁掉/盖住也算丢
  state.ok = state.present && (!shown || (!state.hidden && state.in_right && state.hit !== false));
  return state;
}

let consoleButtonMissingSince = 0;
let consoleButtonSignature = '';
let consoleButtonTimer = 0;
function auditConsoleButton(reason = '') {
  let state;
  try { state = consoleButtonState(); } catch { return; }
  const content = () => {
    const head = $('#detail > .dhead');
    return {dhead: head ? head.outerHTML.slice(0, 12000) : null,
      detail: $('#detail')?.innerHTML.slice(0, 2000) || null};
  };
  const term = typeof T === 'undefined' ? null : {
    name: T.name, uid: T.uid, mode: T.mode, views: [...T.views.keys()],
    visible: !$('#termpane')?.classList.contains('hidden'),
  };
  if (!state.ok) {
    // 丢失期间最多 5 秒记一次，免得 MutationObserver/定时器把库刷爆
    const now = Date.now();
    if (consoleButtonMissingSince && now - consoleButtonMissingSince < 5000) return;
    consoleButtonMissingSince = consoleButtonMissingSince || now;
    browserAuditEvent('console.button.missing', {reason, ...state, selected: S.sel, agent: S.agent,
      terminal: term}, content(), {severity: 'error'});
    return;
  }
  if (consoleButtonMissingSince) {
    browserAuditEvent('console.button.restored', {reason, ...state,
      missing_ms: Date.now() - consoleButtonMissingSince, terminal: term}, null, {severity: 'warning'});
    consoleButtonMissingSince = 0;
  }
  const signature = JSON.stringify([state.label, state.on, state.unavailable, state.rect, state.tier,
    state.hit_target, state.right_scroll]);
  if (signature === consoleButtonSignature) return;
  consoleButtonSignature = signature;
  browserAuditEvent('console.button.state', {reason, ...state, terminal: term});
}
function scheduleConsoleButtonAudit(reason = '') {
  clearTimeout(consoleButtonTimer);
  consoleButtonTimer = setTimeout(() => auditConsoleButton(reason), 150);
}
{
  // #detail 换内容（读取中/失败/正式渲染/新会话页）时重新盯住新的标题栏；标题栏
  // 内部的增删和 class/hidden/style 变化也触发检查。#msgs 的海量变动不在观察范围内。
  const headObserver = new MutationObserver(() => scheduleConsoleButtonAudit('head-mutation'));
  let observedHead = null;
  const watchHead = () => {
    const head = $('#detail > .dhead');
    if (head === observedHead) return;
    headObserver.disconnect();
    observedHead = head;
    if (head) headObserver.observe(head, {childList: true, subtree: true, attributes: true,
      attributeFilter: ['class', 'hidden', 'style', 'aria-label']});
  };
  new MutationObserver(() => { watchHead(); scheduleConsoleButtonAudit('detail-mutation'); })
    .observe($('#detail'), {childList: true});
  watchHead();
  // 裁切/滚走这类不改 DOM 的情况靠 #right 的 scroll 和低频巡检兜底
  $('#right')?.addEventListener('scroll', () => scheduleConsoleButtonAudit('right-scroll'), {passive: true});
  setInterval(() => { if (!document.hidden) auditConsoleButton('periodic'); }, 5000);
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden) scheduleConsoleButtonAudit('visible');
  });
}

/** #detail 每次换内容都记一笔：谁换的、换完有没有标题栏和控制台按钮。 */
function auditDetailRendered(source, extra = {}) {
  const detail = $('#detail');
  browserAuditEvent('detail.rendered', {
    source, selected: S.sel, agent: S.agent, ...extra,
    has_head: !!detail?.querySelector(':scope > .dhead'), has_console_button: !!$('#a-term'),
    children: detail ? [...detail.children].map(
      node => node.id || node.className.split(' ')[0] || node.tagName.toLowerCase()).slice(0, 8) : null,
  });
}

queueMicrotask(() => browserAuditEvent('page.loaded', {
  url: location.pathname + location.search, referrer: document.referrer,
  user_agent: navigator.userAgent, language: navigator.language,
  viewport: {width: innerWidth, height: innerHeight}, theme: document.documentElement.dataset.theme,
}));
window.addEventListener('error', event => browserAuditEvent('error', {
  message: event.message, filename: event.filename, line: event.lineno, column: event.colno,
}, null, {severity: 'error'}));
window.addEventListener('unhandledrejection', event => browserAuditEvent('unhandledrejection', {
  reason: String(event.reason?.stack || event.reason || 'unknown'),
}, null, {severity: 'error'}));
window.addEventListener('online', () => browserAuditEvent('network.online'));
window.addEventListener('offline', () => browserAuditEvent('network.offline', {}, null,
  {severity: 'warning'}));
window.addEventListener('pagehide', () => {
  const snapshot = browserStateSnapshot('pagehide');
  browserAuditEvent('page.hidden', snapshot.data, snapshot.content);
  flushBrowserAuditBeacon();
});
document.addEventListener('visibilitychange', () => {
  browserAuditEvent('visibility.changed', {visibility: document.visibilityState});
  if (!document.hidden) scheduleBrowserSnapshot('visible');
});
document.addEventListener('click', event => {
  const target = event.target?.closest?.(
    'button, a, [role="button"], .item, .ghead, summary, label, input, select, textarea, [data-action], [data-term-key], [data-term-modifier], .fold-preview, .turn-toolbar');
  if (!target) return;
  browserAuditEvent('ui.clicked', {
    tag: target.tagName, id: target.id || '', classes: target.className || '',
    title: target.getAttribute('title') || '', uid: target.dataset?.uid || '',
    action: target.dataset?.v || target.dataset?.termKey || target.dataset?.attach || '',
    text: (target.textContent || '').trim().slice(0, 80),
    x: Math.round(event.clientX), y: Math.round(event.clientY),
  });
}, true);
let auditResizeTimer = 0;
window.addEventListener('resize', () => {
  clearTimeout(auditResizeTimer);
  auditResizeTimer = setTimeout(() => {
    browserAuditEvent('viewport.resized', {
      width: innerWidth, height: innerHeight,
      visual_width: window.visualViewport?.width,
      visual_height: window.visualViewport?.height,
    });
    scheduleBrowserSnapshot('resized');
  }, 200);
});
const el = (tag, cls, html) => {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (html != null) n.innerHTML = html;
  return n;
};
const esc = s => String(s ?? '').replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const icon = src => `<svg class="ico source-icon" data-source="${src}" aria-hidden="true" style="color:${SOURCES[src].color}"><use href="#${SOURCES[src].icon}"/></svg>`;
// 会话头的图标：右上角的运行点和左栏列表一致（绿=直接进程，蓝=受管终端）
const liveStatusTitle = tmux => !SessionDockCapabilities.allows('live') ? '运行状态未知，尚未实现进程探测'
  : tmux ? '运行于受管终端' : '运行中';
const sessionIconMarkup = (src, live, tmux, turn = '', uid = S.sel) => {
  const frozen = sessionFrozen(uid);
  const label = frozen ? '会话已暂停' : liveStatusTitle(tmux) + turnLabel(turn);
  return `<span class="ico">${icon(src)}<span
    class="item-status${live || frozen ? ' visible' : ''}${tmux ? ' tmux' : ''}${frozen ? ' frozen' : turn ? ` turn-${turn}` : ''}" id="dlive"
    title="${esc(label)}" aria-label="${esc(label)}">${frozen ? uiIcon('pause') : ''}</span></span>`;
};
const uiIcon = name => `<svg class="ui-icon" aria-hidden="true"><use href="#i-${name}"/></svg>`;

let staleBuildShown = false;
function markStaleBuild(serverBuild = '') {
  if (staleBuildShown) {
    // “稍后”只收起提示；发送仍被禁用，回到页面时再提醒一次。
    const shown = $('.version-stale');
    if (shown?.hidden && !document.hidden) shown.hidden = false;
    return;
  }
  staleBuildShown = true;
  SessionDockNetwork.pause('stale');
  document.body.classList.add('stale-build');
  const notice = el('div', 'app-float warn version-stale');
  notice.setAttribute('role', 'alert');
  notice.innerHTML = '<div class="app-float-head"><strong>SessionDock 已更新</strong></div>'
    + '<span>自动同步已暂停，仍可编辑并自动保存草稿；发送前请重新加载。</span>';
  browserAuditEvent('build.stale', {server_build: serverBuild});
  const reload = el('button', 'btn primary', '重新加载');
  reload.dataset.act = 'reload';
  reload.type = 'button';
  reload.title = serverBuild ? `服务器版本 ${serverBuild}` : '加载新版本';
  reload.onclick = async () => {
    reload.disabled = true;
    reload.textContent = '正在保存草稿…';
    try {
      if (typeof prepareComposerReload === 'function' && !await prepareComposerReload()) {
        notice.querySelector('span').textContent = '草稿尚未保存或附件尚未上传完成，已取消重新加载。请等待保存成功或保留内容后重试。';
        return;
      }
      location.reload();
    } finally {
      reload.disabled = false;
      reload.textContent = '重新加载';
    }
  };
  const later = el('button', 'btn', '稍后');
  later.type = 'button';
  later.onclick = () => { notice.hidden = true; };
  const actions = el('div', 'app-float-actions');
  actions.append(later, reload);
  notice.appendChild(actions);
  floatStack().appendChild(notice);
  for (const sel of ['#csend', '#bug-report-go', '#cadd', '#bug-report-add']) {
    const button = $(sel);
    if (button) button.disabled = true;
  }
}

let serverHostname = '';
async function checkServerBuild() {
  if (SessionDockNetwork.paused) return;
  try {
    const response = await fetch(appUrl('api/meta'), {cache: 'no-store'});
    const data = await response.json();
    if (data.hostname) serverHostname = data.hostname;
    if (data.build && BUILD_ID && data.build !== BUILD_ID) markStaleBuild(data.build);
  } catch { /* 网络恢复后再检查 */ }
}
setInterval(checkServerBuild, 30000);
queueMicrotask(checkServerBuild);

// 登录环境：后端经 zshrc 之类的包装启动，新开的 CLI 继承后端的环境。启动文件改动后，
// 要重启后端才会进入新会话；后端比对出变化的变量名，这里按机器列成表。
const shellEnvIgnored = new Map();    // 机器 → 本页已忽略的变化（变量名列表）
const shellEnvRestarting = new Map(); // 机器 → {started_at: 发起重启前的启动时间, at: 发起时刻}
const SHELL_ENV_RESTART_WAIT_MS = 120000;
let shellEnvRun = 0;
let shellEnvShown = [];
function shellEnvTargets() {
  if (!HUB_MODE) return [{key: 'local', name: serverHostname || Nodes.list[0]?.name || '本机', prefix: ''}];
  // 正在重启的机器暂时离线也要继续问，直到它带着新的启动时间回来。
  return Nodes.list.filter(node => node.online !== false || shellEnvRestarting.has(node.id))
    .map(node => ({key: node.id, name: node.name || node.id, prefix: `api/nodes/${node.id}/`}));
}

async function checkShellEnv() {
  const run = ++shellEnvRun;
  const targets = shellEnvTargets();
  const found = await Promise.all(targets.map(async target => {
    let data = null;
    try {
      const response = await fetch(appUrl(target.prefix + 'api/shell-env'), {cache: 'no-store'});
      if (response.ok) data = await response.json();   // 旧后端没有这个接口
    } catch { /* 机器重启或离线 */ }
    const restarting = shellEnvRestarting.get(target.key);
    if (restarting) {
      const back = data && data.started_at && data.started_at !== restarting.started_at;
      if (!back && Date.now() - restarting.at < SHELL_ENV_RESTART_WAIT_MS) {
        return {...target, data: data || {}, restarting: true};
      }
      shellEnvRestarting.delete(target.key);
    }
    if (!data?.configured || !data.stale) return null;
    return shellEnvIgnored.get(target.key) === (data.changed || []).join(',') ? null : {...target, data};
  }));
  if (run !== shellEnvRun) return;   // 更新的一轮检查会画出更新的结果
  renderShellEnvNotice(found.filter(Boolean));
}

const textEl = (tag, cls, text) => { const node = el(tag, cls); node.textContent = text; return node; };
function shellEnvButton(label, aria, onclick) {
  const button = textEl('button', 'btn', label);
  button.type = 'button';
  if (aria) button.setAttribute('aria-label', aria);
  button.onclick = onclick;
  return button;
}

function renderShellEnvNotice(items) {
  shellEnvShown = items;
  let notice = $('#shell-env-notice');
  if (!items.length) { notice?.remove(); return; }
  if (!notice) {
    notice = el('div', 'app-float warn shell-env-stale');
    notice.id = 'shell-env-notice';
    notice.setAttribute('role', 'status');
    floatStack().appendChild(notice);
  }
  const pending = items.filter(item => !item.restarting);
  const head = el('div', 'app-float-head');
  head.appendChild(textEl('strong', '', '登录环境（zshrc）已变化'));
  if (pending.length > 1) {
    head.appendChild(shellEnvButton(`全部重启 (${pending.length})`, '', event => restartShellEnv(pending, event.currentTarget)));
  }
  const close = shellEnvButton('×', '关闭', () => {
    for (const item of pending) shellEnvIgnored.set(item.key, (item.data.changed || []).join(','));
    renderShellEnvNotice(shellEnvShown.filter(item => item.restarting));
  });
  close.className = 'modal-close';
  close.title = '忽略这些变化';
  head.appendChild(close);
  const note = textEl('span', '', '新开的会话仍用旧环境，重启后端后生效；正在运行的会话不受影响。');
  const table = el('table', 'shell-env-table');
  const header = table.createTHead().insertRow();
  for (const title of ['机器', '变化', '', '']) header.appendChild(textEl('th', '', title));
  const body = table.createTBody();
  for (const item of items) {
    const row = body.insertRow();
    row.dataset.node = item.key;
    row.appendChild(textEl('td', 'shell-env-node', item.name));
    const names = item.data.changed || [];
    row.appendChild(textEl('td', 'shell-env-changed', item.restarting ? '正在重启…' : names.join('、')));
    const restart = row.insertCell(), ignore = row.insertCell();
    if (item.restarting) continue;
    restart.appendChild(shellEnvButton('重启', `重启 ${item.name} 后端`, event => restartShellEnv([item], event.currentTarget)));
    ignore.appendChild(shellEnvButton('忽略', `忽略 ${item.name}`, () => {
      shellEnvIgnored.set(item.key, names.join(','));
      renderShellEnvNotice(shellEnvShown.filter(other => other.key !== item.key));
    }));
  }
  notice.replaceChildren(head, note, table);
}

async function restartShellEnv(items, button) {
  button.disabled = true;
  const failed = [];
  await Promise.all(items.map(async item => {
    try {
      const response = await fetch(appUrl(item.prefix + 'api/shell-env/restart'), {method: 'POST'});
      const data = await response.json().catch(() => ({}));
      if (!response.ok || data.error) throw new Error(data.error || `HTTP ${response.status}`);
      shellEnvRestarting.set(item.key, {started_at: item.data.started_at, at: Date.now()});
      browserAuditEvent('shell_env.restart', {node: item.key, changed: item.data.changed || []});
    } catch (error) {
      failed.push(`${item.name}: ${error.message || error}`);
    }
  }));
  // 先原地把这几行换成“正在重启…”，其他机器的行保持不动。
  renderShellEnvNotice(shellEnvShown.map(item =>
    shellEnvRestarting.has(item.key) ? {...item, restarting: true} : item));
  if (failed.length) appAlert(`重启后端失败:\n${failed.join('\n')}`);
  for (let i = 0; i < 60 && items.some(item => shellEnvRestarting.has(item.key)); i++) {
    await new Promise(resolve => setTimeout(resolve, 2000));
    await checkShellEnv();
  }
}
setInterval(checkShellEnv, 60000);
setTimeout(checkShellEnv, 3000);
document.addEventListener('visibilitychange', () => {
  if (!document.hidden) checkShellEnv();
});
document.addEventListener('visibilitychange', () => {
  if (!document.hidden) checkServerBuild();
});

function browserPinchZoomed() { return SessionDockShell.browserPinchZoomed(); }
function visualKeyboardOpen() { return SessionDockShell.visualKeyboardOpen(); }
function measureKeyboardClosedLayout(measure) { return SessionDockShell.measureKeyboardClosedLayout(measure); }
function syncMobileViewport() { SessionDockShell.syncMobileViewport(); }
function showMobileDetail() { SessionDockShell.showMobileDetail(); }
function showMobileList() { SessionDockShell.showMobileList(); }

function fmtSize(n) {
  if (n < 1024) return n + 'B';
  if (n < 1048576) return (n / 1024).toFixed(0) + 'K';
  return (n / 1048576).toFixed(1) + 'M';
}
function fmtTime(iso) {
  if (!iso) return '';
  const d = new Date(iso), now = new Date();
  const p = n => String(n).padStart(2, '0');
  const hm = `${p(d.getHours())}:${p(d.getMinutes())}`;
  if (d.toDateString() === now.toDateString()) return '今天 ' + hm;
  const y = new Date(now - 86400000);
  if (d.toDateString() === y.toDateString()) return '昨天 ' + hm;
  if (d.getFullYear() === now.getFullYear()) return `${d.getMonth() + 1}-${p(d.getDate())} ${hm}`;
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}
/** 起止时间：同一天只写一次日期前缀；没有结束时间就是还在跑。 */
function fmtSpan(start, end) {
  const from = fmtTime(start);
  if (!end) return from ? `${from} → 运行中` : '运行中';
  const to = fmtTime(end);
  if (!from) return to;   // 旧节点的列表项还没有开始时间
  const split = s => { const i = s.lastIndexOf(' '); return i < 0 ? [s, ''] : [s.slice(0, i), s.slice(i + 1)]; };
  const [fromDay, fromClock] = split(from), [toDay, toClock] = split(to);
  return fromClock && toClock && fromDay === toDay ? `${from} → ${toClock}` : `${from} → ${to}`;
}
/** 家目录缩成 ~; 过长的路径中间省略, 首尾都是有信息量的部分。
 *  不能用 CSS direction:rtl 来截左边 —— bidi 会把开头的 "/" 挪到末尾。 */
function shortCwd(p, max = 40) {
  p = (p || '(未知)').replace(/^\/home\/[^/]+/, '~');
  if (p.length <= max) return p;
  const seg = p.split('/');
  return seg.length > 3 ? `${seg[0]}/${seg[1]}/…/${seg.slice(-2).join('/')}` : p;
}

const timelinePath = cwd => (cwd || '(未知)').replace(/^\/home\/[^/]+/, '~').replace(/\/+$/, '') || '/';
const TIMELINE_COLORS = ['blue', 'teal', 'violet', 'amber', 'rose', 'olive', 'rust', 'cyan'];
let timelineColors = new Map();
try {
  for (const [path, color] of store.get('timelineDirectoryColors', [])) {
    if (typeof path === 'string' && TIMELINE_COLORS.includes(color)
        && ![...timelineColors.values()].includes(color)) timelineColors.set(path, color);
  }
} catch { /* Ignore invalid saved assignments. */ }

function timelineDirectoryColors(rows) {
  const counts = new Map(), seen = new Set();
  for (const row of rows) {
    if (!row.cwd || row.cwd === '(未知)' || seen.has(row.uid)) continue;
    seen.add(row.uid); // Search results also contain sessions in the main list.
    const path = timelinePath(row.cwd);
    counts.set(path, (counts.get(path) || 0) + 1);
  }
  const common = [...counts].filter(([, count]) => count >= 2)
    .sort(([a, ac], [b, bc]) => bc - ac || (a < b ? -1 : a > b ? 1 : 0))
    .slice(0, TIMELINE_COLORS.length).map(([path]) => path);
  const assigned = new Map(common.filter(path => timelineColors.has(path))
    .map(path => [path, timelineColors.get(path)]));
  const free = TIMELINE_COLORS.filter(color => ![...assigned.values()].includes(color));
  for (const path of common) {
    if (!assigned.has(path)) assigned.set(path, free.shift());
  }
  // Preserve slots when session counts reorder the common directories. Filters
  // use the full pool, and reloads reuse the assignment instead of recoloring it.
  if (assigned.size !== timelineColors.size
      || [...assigned].some(([path, color]) => timelineColors.get(path) !== color)) {
    timelineColors = assigned;
    store.set('timelineDirectoryColors', [...assigned]);
  }
  return timelineColors;
}

/** Count directories, not sessions: a busy project must not change which parts
 *  identify a path. Keep the full pool even while filtering the sidebar. */
let timelinePlanCache = {paths: new Set(), plans: new Map()};
function timelinePathPlans(rows) {
  const pathSet = new Set(rows.map(s => timelinePath(s.cwd)));
  if (pathSet.size === timelinePlanCache.paths.size
      && [...pathSet].every(path => timelinePlanCache.paths.has(path))) return timelinePlanCache.plans;
  const paths = [...pathSet];
  const frequency = new Map(), peers = new Map();
  for (const path of paths) {
    const parts = path.split('/'), leaf = parts.at(-1);
    for (const part of new Set(parts)) frequency.set(part, (frequency.get(part) || 0) + 1);
    // Any matching abbreviation must contain every literal component. Index
    // those components so same-basename temporary directories do not require
    // an all-pairs regex scan (their distinguishing component is often unique).
    if (!peers.has(leaf)) peers.set(leaf, new Map());
    const components = peers.get(leaf);
    for (const part of new Set(parts)) {
      if (!components.has(part)) components.set(part, new Set());
      components.get(part).add(path);
    }
  }
  const plans = new Map(paths.map(path => {
    const parts = path === '/' ? ['/'] : path.split('/');
    const leaf = parts.at(-1), hidden = new Set(), labels = [path];
    // Repeated ancestors carry less information. Ties favor keeping the deeper
    // context. Never remove the root anchor or any part of the final directory.
    const order = parts.map((_, i) => i).slice(1, -1).sort((a, b) =>
      frequency.get(parts[b]) - frequency.get(parts[a]) || a - b);
    for (const i of order) {
      hidden.add(i);
      const tokens = parts.flatMap((part, j) => hidden.has(j)
        ? (hidden.has(j - 1) ? [] : [null]) : [part]);
      // An ellipsis represents whole directories. If it could also stand for
      // another path with the same basename, retain the distinguishing ancestor.
      const components = peers.get(leaf);
      const candidates = tokens.filter(token => token !== null)
        .map(token => components.get(token)).reduce((a, b) => a.size <= b.size ? a : b);
      const pattern = candidates.size > 1 ? new RegExp('^' + tokens.map(token => token === null
        ? '(?:[^/]+/)*[^/]+' : token.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('/') + '$') : null;
      if (pattern && [...candidates].some(other => other !== path && pattern.test(other))) {
        hidden.delete(i);
        continue;
      }
      const label = tokens.map(token => token === null ? '…' : token).join('/');
      labels.push(label);
    }
    return [path, {leaf, labels}];
  }));
  timelinePlanCache = {paths: pathSet, plans};
  return plans;
}

function timelinePathMarkup(path, leaf) {
  return esc(path.slice(0, path.length - leaf.length))
    + `<span class="cwd-leaf">${esc(leaf)}</span>`;
}

function timelineDirectoryMarkup(row) {
  const path = timelinePath(row.cwd), leaf = path === '/' ? '/' : path.split('/').at(-1);
  return (row.node_name ? `<span class="cwd-machine">${nodeBadge(row.node_name)}</span>` : '')
    + `<span class="cwd-path" data-path="${esc(path)}">${timelinePathMarkup(path, leaf)}</span>`;
}

let timelineFitContext = null;
function fitTimelineDirectories(elements = null, context = null) {
  if (S.view !== 'date') return;
  elements ||= [...document.querySelectorAll('#side .cwd-path')];
  if (!elements.length) return;
  if (!context) {
    const rows = [...S.sessions, ...pendingTmuxSessions(), ...(S.results || [])];
    context = timelineFitContext = {plans: timelinePathPlans(rows), colors: timelineDirectoryColors(rows)};
  }
  const {plans, colors} = context;
  const measure = document.createElement('canvas').getContext('2d');
  const font = getComputedStyle(elements[0]);
  measure.font = `${font.fontWeight} ${font.fontSize} ${font.fontFamily}`;
  const widths = new Map();
  // Batch layout reads before writes; repeated rows share measured labels.
  const updates = elements.map(element => {
    const plan = plans.get(element.dataset.path);
    if (!plan) return null;
    const color = colors.get(element.dataset.path) || '';
    const width = element.clientWidth;
    if (!width) return {element, color}; // Fit a closed date group when opened.
    const measured = label => {
      if (!widths.has(label)) {
        widths.set(label, measure.measureText(label).width);
      }
      return widths.get(label);
    };
    const label = plan.labels.find(label => measured(label) <= width)
      || plan.labels.reduce((best, label) => measured(label) < measured(best) ? label : best);
    return {element, color, label};
  });
  if (sidebarVueMounted) SessionDockSidebar.fitPaths(updates.filter(Boolean).map(update => ({
    key: update.element.closest('.item').dataset.key, color: update.color, label: update.label,
  })));
}

let timelineFitFrame = 0;
function scheduleTimelineFit() {
  cancelAnimationFrame(timelineFitFrame);
  timelineFitFrame = requestAnimationFrame(() => fitTimelineDirectories());
}
new ResizeObserver(scheduleTimelineFit).observe($('#side'));
document.fonts.ready.then(scheduleTimelineFit);

const dayKey = iso => {
  const d = new Date(iso), p = n => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
};

// ---------------------------------------------------------------- 消息缓存
// 一次读完整个会话, 结果按 LRU 留在内存; 会话是 append-only 的,
// 再次打开时只向服务端要新追加的部分。
let cacheLimitMb = Math.max(0, +store.get('cacheMb', 256) || 0);
let CACHE_MAX_BYTES = cacheLimitMb ? cacheLimitMb * 1024 * 1024 : Infinity;
const RENDER_BATCH = 250;
const SYNC_MS = 10000;      // 没在运行的会话, 偶尔看一眼就行
const LIVE_MS = 3000;       // 活跃探测(扫 /proc)的间隔
// 正在看的活跃会话用自适应间隔: 有新内容就贴到最快, 静下来逐步退避
const FAST_MIN = 350;
const FAST_MAX = 3000;
const TICK_MS = 200;
const BACKUP_MS = 20000;    // SSE 正常时的兜底对账间隔
const LIST_MS = 8000;       // 会话列表跟进磁盘变化的间隔
// 主动增量读取可能因浏览器连接池、网络切换或代理半开而既不成功也不失败。
// 只要响应头或正文仍有进展就续期；真正静止到这个时长才中止，让下一次
// SSE/对账从同一游标重试。用 let 是为了浏览器 E2E 能把分钟级故障压缩到毫秒。
let SYNC_STALL_MS = 12000;
const cache = new Map();          // viewKey → {meta, msgs, version, end, bytes}
const {messageIndexes,messageIndex}=SessionDockConversation.index;
// 多题题卡会被 SSE、兜底对账和完整重绘反复替换 DOM。未提交选择必须独立于
// 节点保存，否则下一次后台刷新就会让用户刚点的答案消失。
const questionFormDrafts = new Map(); // `${uid}\0${tool id}` → option index[]
const viewKey = (uid, agent = null) => agent ? `${uid}::${agent}` : uid;
const INCOMING_ROLES = new Set([
  'assistant', 'assistant·subagent', 'thinking', 'tool', 'tool_result', 'question',
]);
const incomingCount = msgs => msgs.filter(m => m.counted !== false && INCOMING_ROLES.has(m.role)).length;
const messageCount = msgs => msgs.filter(m => m.counted !== false).length;
const entryTotal = entry => Number.isFinite(+entry?.total)
  ? +entry.total : messageCount(entry?.msgs || []);

function queuedAfterTimestamp(uid) {
  const entry = cache.get(viewKey(uid));
  let latest = Date.parse(entry?.activity?.ts || '');
  for (let i = (entry?.msgs?.length || 0) - 1; i >= 0; i--) {
    const at = Date.parse(entry.msgs[i]?.ts || '');
    if (!Number.isFinite(at)) continue;
    latest = Number.isFinite(latest) ? Math.max(latest, at) : at;
    break;
  }
  return Number.isFinite(latest) ? new Date(latest).toISOString() : null;
}

function cacheGet(uid) {
  const e = cache.get(uid);
  if (e) { cache.delete(uid); cache.set(uid, e); }   // 命中即移到队尾
  return e;
}

function cacheEntryUid(key, entry) {
  // 子代理视图的 key 是 uid::agent，但它和主会话共用同一个 tmux 生命周期。
  return entry?.meta?.uid || String(key).split('::', 1)[0];
}

function cacheEntryPinned(key, entry) {
  const uid = cacheEntryUid(key, entry);
  // 详情 DOM 直接由当前缓存生成。若容量整理把正在看的这一份删掉，页面仍
  // 看似正常，却再也没有游标可供 SSE/outbox 补读，最终会留下永久 pending。
  return key === viewKey(S.sel, S.agent)
    || S.liveTmux.has(uid)
    || (typeof T !== 'undefined' && T.list?.some(x => x.uid === uid));
}

function trimCache() {
  // tmux 对话必须随时切回即见，因此不计入容量、也不参与淘汰。
  // 普通历史对话单独共享用户设置的容量，并延续“至少保留最新一份”的旧行为。
  const evictable = [...cache].filter(([key, entry]) => !cacheEntryPinned(key, entry));
  let total = evictable.reduce((n, [, entry]) => n + (+entry.bytes || 0), 0);
  let remaining = evictable.length;
  for (const [key, entry] of evictable) {
    if (total <= CACHE_MAX_BYTES || remaining <= 1) break;
    cache.delete(key);
    total -= +entry.bytes || 0;
    remaining--;
  }
}

function cachePut(uid, e) {
  cache.delete(uid);
  cache.set(uid, e);
  trimCache();
}

/** 带下载进度的取消息。start/head 给定时服务端只回新增部分。 */
async function fetchMessages(uid, opts = {}) {
  const p = new URLSearchParams();
  if (opts.start) {
    p.set('start', opts.start);
    p.set('head', opts.head);
    p.set('anchor', opts.anchor || '');     // 没有锚点服务端会拒绝续读, 直接给整份
  }
  if (opts.agent) p.set('agent', opts.agent);
  if (opts.appendOnly) p.set('append', '1');
  if (opts.windowed) p.set('window', '1');
  const traceId = globalThis.crypto?.randomUUID?.()
    || `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const url = `api/messages/${encodeURIComponent(uid)}?${p}`;
  const started = performance.now();
  browserAuditEvent('http.request.started', {
    url, method: 'GET', start: opts.start || 0, agent: opts.agent || '',
  }, null, {uid, traceId});
  let r;
  try {
    r = await fetch(appUrl(url), {
      signal: opts.signal,
      headers: {'X-SessionDock-Trace': traceId, 'X-SessionDock-Page': AUDIT_PAGE_ID,
        'X-SessionDock-Build': BUILD_ID},
    });
  } catch (error) {
    browserAuditEvent('http.request.failed', {
      url, error: String(error?.name || error),
      duration_ms: Math.round((performance.now() - started) * 1000) / 1000,
    }, null, {uid, traceId, severity: error?.name === 'AbortError' ? 'warning' : 'error'});
    throw error;
  }
  opts.onActivity?.();
  if (!r.ok) {
    browserAuditEvent('http.response.received', {url, status: r.status, ok: false},
      null, {uid, traceId, severity: 'warning'});
    const detail = SessionDockCapabilities.config.backend === 'rust'
      ? await r.json().catch(() => null) : null;
    const error = new Error(detail?.error || 'HTTP ' + r.status);
    error.status = r.status;
    error.code = detail?.code || '';
    throw error;
  }
  // Fetch 自动解压 gzip：reader.read() 统计的是解压后字节，而标准
  // Content-Length 仍可能是压缩后大小。优先用服务端给出的同口径长度；
  // 连到旧服务端时，压缩响应改显示不定进度，也不伪造一个较小的分母。
  const contentTotal = +r.headers.get('Content-Length') || 0;
  const decodedTotal = +r.headers.get('X-SessionDock-Decoded-Length') || 0;
  const encoded = !!r.headers.get('Content-Encoding');
  const total = decodedTotal || (encoded ? 0 : contentTotal);
  const reader = r.body.getReader();
  const chunks = [];
  let got = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    got += value.length;
    opts.onActivity?.();
    if (encoded && contentTotal && decodedTotal) {
      // Fetch 不暴露实时压缩字节数。用解压进度映射到已知的
      // 压缩总量：中途值明确标为估算，最后一帧则精确等于响应体流量。
      const transferred = Math.min(contentTotal,
        Math.round(got / decodedTotal * contentTotal));
      opts.onProgress?.(transferred, contentTotal, {
        compressed: true, estimated: got < decodedTotal,
      });
    } else {
      opts.onProgress?.(got, total);
    }
  }
  const buf = new Uint8Array(got);
  let at = 0;
  for (const c of chunks) { buf.set(c, at); at += c.length; }
  const data = JSON.parse(new TextDecoder().decode(buf));
  browserAuditEvent('http.response.parsed', {
    url, status: r.status, bytes: got, reset: !!data.reset,
    start: data.start, end: data.end, messages: data.messages?.length || 0,
    duration_ms: Math.round((performance.now() - started) * 1000) / 1000,
  }, null, {uid, traceId});
  return { data, bytes: got,
    networkBytes: encoded && contentTotal ? contentTotal : got };
}

// ---- 进度条 ----
function progress(done, total, label, detail = {}) {
  const bar = $('#prog');
  bar.classList.add('on');
  const pct = total ? Math.min(100, done / total * 100) : 0;
  bar.querySelector('.bar').style.width = (total ? pct : 12) + '%';
  bar.querySelector('.bar').classList.toggle('idle', !total);
  const estimate = detail.estimated ? '≈' : '';
  const compressed = detail.compressed ? '（压缩）' : '';
  bar.querySelector('.txt').textContent = total
    ? `${label} ${label === '渲染' ? `${done}/${total}`
      : estimate + fmtSize(done) + ' / ' + fmtSize(total) + compressed}`
    : `${label}…`;
}

function progressDone() {
  const bar = $('#prog');
  bar.classList.remove('on');
  bar.querySelector('.bar').style.width = '0%';
}

/** 把服务端给的一份 diff 应用到缓存和界面上。
 *  两种情况: 追加(接到末尾) 或 reset(整份重来) —— 和服务端的判定一一对应。 */
function normalizedQuestionAnswer(text) {
  const raw = String(text || '');
  if (/^aborted by user(?:\s|$)/i.test(raw.trim())
      || raw.startsWith("The user doesn't want to proceed with this tool use.")) {
    return '已取消回答';
  }
  try {
    const answers = JSON.parse(raw)?.answers;
    if (!answers || typeof answers !== 'object') return raw;
    const rows = Object.values(answers).map(answer => {
      const values = answer && typeof answer === 'object' ? answer.answers : answer;
      return Array.isArray(values) ? values.join('、') : String(values || '').trim();
    }).filter(Boolean);
    return rows.join('\n') || raw;
  } catch { return raw; }
}

/** 正文游标可能已经由并行 fetch 推进，但后到的 SSE 仍可能携带更新的活动态。
 *  活动态有自己的时间线：接收较新的状态，绝不让旧 Working 覆盖已中断。 */
function activityFollows(current, incoming) {
  if (!current) return true;
  if (!incoming) return ['working', 'waiting'].includes(current.state);
  const currentAt = Date.parse(current.ts || '');
  const incomingAt = Date.parse(incoming.ts || '');
  if (Number.isFinite(currentAt) && Number.isFinite(incomingAt)) {
    return incomingAt >= currentAt;
  }
  const currentBusy = ['working', 'waiting'].includes(current.state);
  const incomingBusy = ['working', 'waiting'].includes(incoming.state);
  if (currentBusy !== incomingBusy) return currentBusy && !incomingBusy;
  return true;
}

function applyCoveredActivity(uid, agent, entry, data) {
  if (!data.activity_changed || !activityFollows(entry.activity, data.activity)) return;
  entry.activity = data.activity;
  markInterruptedTurn(entry.msgs, entry.activity);
  if (S.sel !== uid || S.agent !== agent) return;
  const box = $('#msgs');
  if (box && entry.activity?.state !== 'working') sealTurnTail(box, entry);
  renderConversationTail(entry.activity, uid);
}

function applyMigrationMeta(uid, agent, entry, meta) {
  if (SessionDockCapabilities.config.backend !== 'rust' || !meta
      || meta.uid !== uid || (meta.agent_id || null) !== agent) return;
  const key = m => JSON.stringify([m.title, m.parent_title, m.sid, m.agent_type,
    m.cwd, m.model, !!m.starred, m.fork_parent_visible,
    m.nest_parent || null, m.group || null,
    (m.agent_items || []).map(a => [a.id, a.title, a.type])]);
  const changed = key(entry.meta) !== key(meta);
  entry.meta = meta;
  if (!agent) {
    const listed = indexedSessions().byUid.get(uid);
    // Cursor/size updates do not change membership, ordering, nesting or
    // filters; migration_warnings 同样只出现在详情里，左栏从不展示（docs/history-pages.md）。
    // Keep the indexed row object used by the resolved sidebar tree;
    // replace the array normally for every other kind of metadata change.
    if (listed && JSON.stringify(listed) !== JSON.stringify(meta)) {
      const local = Object.keys({...listed, ...meta}).every(field =>
        field === 'cursor' || field === 'size' || field === 'migration_warnings'
          || JSON.stringify(listed[field]) === JSON.stringify(meta[field]));
      if (local) {
        const sizeChanged = listed.size !== meta.size;
        for (const field of ['cursor', 'size', 'migration_warnings']) {
          if (Object.hasOwn(meta, field)) listed[field] = meta[field];
          else delete listed[field];
        }
        if (sizeChanged) {
          const node = $('#side').querySelector(`.item:not(.agent)[data-uid="${CSS.escape(uid)}"]`);
          if (node?._nestRow) {
            node._signature = null;
            if (!sidebarTextSelectionProtected()) refreshSidebarRows(uid);
          }
        }
      } else S.sessions = S.sessions.map(row => row.uid === uid ? {...meta} : row);
    }
  }
  if (!changed) {
    // The first native message changes an assigned launch from unused to
    // populated even when its title and other header fields stay the same.
    if (S.sel === uid && !S.agent) renderSessionAction(meta);
    return;
  }
  if (!agent) renderSide();
  if (S.sel !== uid || S.agent !== agent) return;
  const oldHead = $('#detail > .dhead');
  if (!oldHead) return;
  const wasOpen = oldHead.querySelector('#session-view-menu')?.hidden === false;
  const next = head(meta, entryTotal(entry));
  if (wasOpen) {
    SessionDockSessionUi.restoreViews();
  }
  oldHead.replaceWith(next);
  layoutSessionHead();
}

async function applyDiff(uid, data, bytes = 0, agent = null) {
  try { return await applyDiffPacket(uid, data, bytes, agent); }
  // activity 与 CLI 画面都随数据包到达，左栏和会话头的回合状态跟着重画。
  finally { if (!agent) paintTurn(uid); }
}

async function applyDiffPacket(uid, data, bytes = 0, agent = null) {
  const key = viewKey(uid, agent);
  if (migrationReadPaused(uid, agent)) return 0;
  const e = cache.get(key);
  if (!e) return 0;
  if (data.prompt_only) {
    if (!agent && Object.prototype.hasOwnProperty.call(data, 'prompt')) {
      e.prompt = data.prompt || null;
      globalThis.revealConversationForPrompt?.(uid, e.prompt);
    }
    if (S.sel === uid && !S.agent) renderConversationTail(e.activity, uid);
    return 0;
  }
  if (data.cli_only) {
    // The session's CLI state changed (input readiness, editor text, queued
    // sends) without new records (docs/cli-state.md).
    if (!agent && Object.prototype.hasOwnProperty.call(data, 'cli')) {
      e.cli = data.cli || null;
      globalThis.applyCliState?.(uid, e.cli);
    }
    return 0;
  }
  // 正文 diff 受游标约束。SSE 与兜底拉取可能同时从同一旧游标出发；
  // 乱序包必须在修改 outbox、prompt 或乐观消息之前丢弃，否则正文没被
  // 接收，发送占位却已先清掉。
  if (!data.reset && data.start !== e.end) {
    const packetStart = Number(data.start), packetEnd = Number(data.end);
    const currentEnd = Number(e.end);
    if (Number.isFinite(packetStart) && Number.isFinite(packetEnd)
        && Number.isFinite(currentEnd)
        && packetStart < currentEnd && packetEnd <= currentEnd) {
      // SSE 与主动 fetch 从同一旧游标出发时，后到者可能是一份已被前者
      // 完整覆盖的重复正文。活动态使用独立修订，仍须接收其中较新的
      // aborted/failed；否则页面要等兜底拉取才会清掉 Working。
      applyCoveredActivity(uid, agent, e, data);
      return 0;
    }
    scheduleDiffRecovery(uid, agent);
    return 0;
  }
  if (!agent && Object.prototype.hasOwnProperty.call(data, 'prompt')) {
    e.prompt = data.prompt || null;
    globalThis.revealConversationForPrompt?.(uid, e.prompt);
  }
  if (!agent && Object.prototype.hasOwnProperty.call(data, 'cli')) {
    e.cli = data.cli || null;
    globalThis.applyCliState?.(uid, e.cli);
  }
  const questionCalls = data.reset ? new Set() : messageIndex(e.msgs).questions;
  for (const message of data.messages || []) {
    if (message.role === 'question' && message.call_id) questionCalls.add(message.call_id);
  }
  data.messages = (data.messages || []).map(m => {
    if (m.role !== 'tool_result' || !questionCalls.has(m.call_id)) return m;
    return { ...m, role: 'answer', name: m.name || 'AskUserQuestion',
      text: normalizedQuestionAnswer(m.text) };
  });
  if (data.reset) {                         // 回滚 / 重写过, 缓存作废
    markInterruptedTurn(data.messages, data.activity);
    cachePut(key, { meta: data.meta, msgs: data.messages, version: data.version,
                    end: data.end, anchor: data.anchor, activity: data.activity, bytes,
                    prompt: data.prompt || null, cli: data.cli ?? null,
                    total: data.message_total, partial: data.partial || null });
    S.cursors.set(key, { end: data.end, head: data.version.head, anchor: data.anchor });
    if (S.sel === uid && S.agent === agent) {
      // Rust keeps SSE live during explicit window reloads and native resets.
      // Publish the new snapshot together with any suffix/activity accepted
      // while this renderer yields; the existing render path is retained.
      const options = SessionDockCapabilities.config.backend === 'rust'
        ? {historyPageEntry: cache.get(key)} : {};
      await renderSession(data.meta, data.messages, data.activity, options);
    }
    return data.messages.length;
  }
  e.version = data.version;
  e.end = data.end;
  e.anchor = data.anchor;
  S.cursors.set(key, { end: data.end, head: data.version.head, anchor: data.anchor });
  e.bytes += bytes;
  applyMigrationMeta(uid, agent, e, data.meta);
  if (data.activity_changed) {
    e.activity = data.activity;
    // turn_aborted 可能单独成为一个无正文的 SSE 包，也可能与末批正文一起
    // 到达；两边都补标，不能依赖恰好落在同一次 JSONL 增量读取中。
    markInterruptedTurn(e.msgs, e.activity);
    markInterruptedTurn(data.messages, e.activity);
  }
  else if (e.activity?.state === 'waiting' && data.messages.some(
      m => m.role === 'tool_result' || m.role === 'answer')) {
    // 问题和回答可能分属两次增量读取，第二次已没有 call_id 映射。
    const answerAt = data.messages.findIndex(m => m.role === 'tool_result');
    data.messages = data.messages.map((m, i) => i === answerAt && m.role === 'tool_result'
      ? { ...m, role: 'answer' } : m);
    e.activity = { role: 'status', state: 'working', text: 'working', ts: new Date().toISOString() };
  }
  if (!data.messages.length) {
    if (S.sel === uid && S.agent === agent) {
      const box = $('#msgs');
      if (box && e.activity?.state !== 'working') sealTurnTail(box, e);
      renderConversationTail(e.activity, uid);
    }
    return 0;
  }
  e.total = entryTotal(e) + messageCount(data.messages);
  for (const message of data.messages) e.msgs.push(message);
  const incoming = incomingCount(data.messages);
  const detailVisible = S.sel === uid && S.agent === agent
    && (!MOBILE.matches || document.body.classList.contains('mobile-detail'));
  if (incoming && !detailVisible) addUnread(uid, incoming);
  if (S.sel !== uid || S.agent !== agent) return data.messages.length;
  const box = $('#msgs');
  if (!box) return data.messages.length;
  const built = appendMessages(box, data.messages, null,
    {openTail: e.activity?.state === 'working'});
  const sealed = sealTurnTail(box, e);
  if (!sealed) {
    built.forEach(markMatches);
    if (S.term) updateMatchNav();
  }
  renderConversationTail(e.activity, uid);
  const c = $('#mcount-total');
  const total = entryTotal(e);
  if (c) SessionDockSessionUi.messageCount(total);
  const mc = $('.mobile-msg-count');
  if (mc) {
    mc.textContent = total;
    mc.setAttribute('aria-label', `${total} 条消息`);
  }
  return data.messages.length;
}

/** 兜底用的主动拉取。正常情况下更新由服务端 SSE 推过来, 这里只在
 *  连接还没建起来或断了的时候补一手。 */
const syncingViews = new Map();
// Migration-only failures stop background retries for this exact view. Keep
// the last good snapshot; only a successful explicit HTTP retry clears them.
const migrationReadFailures = new Map();
const migrationReadRetries = new Map();
const migrationReadProbes = new Map();

function migrationReadPaused(uid, agent = null) {
  return SessionDockCapabilities.config.backend === 'rust'
    && migrationReadFailures.has(viewKey(uid, agent));
}

/** 瞬时失败只影响这一次请求：网络层错误、中止、408/429、5xx（501 除外）。
 *  它们走退避重试，不暂停视图、不弹横幅、不关 SSE。 */
function transientReadFailure(error) {
  if (!error) return false;
  if (error.name === 'AbortError' || error.name === 'TypeError') return true;
  const status = Number(error.status) || 0;
  return status === 408 || status === 429 || (status >= 500 && status !== 501);
}

/** 只有服务端可能已修复的失败才给“重试读取”：4xx 是请求本身不成立（不存在、
 *  超预算、格式错），重试同一读取不会有不同结果。 */
function retryableReadFailure(failure) {
  const status = Number(failure?.status) || 0;
  return !status || status >= 500;
}

// 指数退避：1.5 s 起，每次翻倍，封顶 15 s；成功后归零。
const RETRY_BASE_MS = 1500;
const RETRY_MAX_MS = 15000;
const retryDelay = attempt => Math.min(RETRY_MAX_MS, RETRY_BASE_MS * 2 ** Math.max(0, attempt));

function renderMigrationReadFailure(uid, agent = null) {
  if (SessionDockCapabilities.config.backend !== 'rust' || S.sel !== uid || S.agent !== agent) return;
  const detail = $('#detail'); if (!detail) return;
  const failure = migrationReadFailures.get(viewKey(uid,agent));
  if (!failure) delete detail.dataset.migrationStale;
  else detail.dataset.migrationStale = 'true';
  SessionDockConversation.readFailure(detail,uid,agent,cache.has(viewKey(uid,agent)) ? failure : null);
}

/** 不可恢复失败的登记：暂停该视图的后台读取与 SSE，保留先前快照。 */
function reportMigrationReadFailure(uid, agent, error) {
  if (SessionDockCapabilities.config.backend !== 'rust') return null;
  const failure = {message: String(error?.message || error?.error || '无法确认最新历史，请重试。'),
    status: Number(error?.status) || 0, code: String(error?.code || '')};
  migrationReadFailures.set(viewKey(uid, agent), failure);
  if (S.sel === uid && S.agent === agent && _esUid === uid) closeWatch();
  renderMigrationReadFailure(uid, agent);
  return failure;
}

/** 请求失败的分流：瞬时失败不登记（返回 null，由调用方按退避重试），
 *  其余交给 reportMigrationReadFailure。 */
function reportReadFailure(uid, agent, error) {
  if (SessionDockCapabilities.config.backend !== 'rust') return null;
  if (transientReadFailure(error)) return null;
  return reportMigrationReadFailure(uid, agent, error);
}

async function retryMigrationRead(uid, agent = null) {
  if (SessionDockCapabilities.config.backend !== 'rust') return false;
  const key = viewKey(uid, agent);
  if (migrationReadRetries.has(key)) return migrationReadRetries.get(key);
  const task = (async () => {
    // Let an already-started request finish before authorizing fresh state.
    await Promise.allSettled([syncingViews.get(key), migrationReadProbes.get(key)].filter(Boolean));
    const failure = migrationReadFailures.get(key);
    if (!failure) return false;
    failure.retrying = true;
    renderMigrationReadFailure(uid, agent);
    const ac = new AbortController();
    const timer = setTimeout(() => ac.abort(), SYNC_STALL_MS);
    try {
      const {data, bytes} = await fetchMessages(uid, {agent, windowed: true, signal: ac.signal});
      if (!data?.meta || !data.version || !Array.isArray(data.messages)
          || !Number.isFinite(data.end)) throw new Error('服务端返回了无效的会话快照');
      cachePut(key, {meta: data.meta, msgs: data.messages, version: data.version,
        end: data.end, anchor: data.anchor, activity: data.activity, bytes,
        prompt: data.prompt || null, cli: data.cli ?? null, total: data.message_total, partial: data.partial || null});
      S.cursors.set(key, {end: data.end, head: data.version.head, anchor: data.anchor});
      migrationReadFailures.delete(key);
      if (S.sel === uid && S.agent === agent) {
        await renderSession(data.meta, data.messages, data.activity, {startWatch: false});
        renderMigrationReadFailure(uid, agent);
        if (S.sel === uid && S.agent === agent) watchSession(uid, agent);
      }
      return true;
    } catch (error) {
      reportMigrationReadFailure(uid, agent, error);
      return false;
    } finally { clearTimeout(timer); }
  })().finally(() => {
    if (migrationReadRetries.get(key) === task) migrationReadRetries.delete(key);
  });
  migrationReadRetries.set(key, task);
  return task;
}

/** 服务端明确的 migration-error 事件：不可恢复时暂停并保留快照。
 *  原生文件并发增长等情况也会以 migration-error 携带 503；它仍是瞬时失败，
 *  留给随后到来的 es.onerror 按退避策略重开。 */
function pauseMigrationWatch(es, uid, agent, error = null) {
  if (SessionDockCapabilities.config.backend !== 'rust' || _es !== es
      || _esUid !== uid || S.sel !== uid || S.agent !== agent) return false;
  if (!error) return false;
  if (transientReadFailure(error)) return false;
  reportMigrationReadFailure(uid, agent, error);
  return true;
}

/** EventSource 藏起了 HTTP 拒绝的正文。流从未打开就失败时，用一次增量读取
 *  探明原因：不可恢复（501 等）就暂停并给出理由；瞬时失败或读取成功则说明
 *  只是流本身没建起来，交回退避重连。探测不改快照、不重开流。 */
function probeWatchRejection(uid, agent) {
  if (SessionDockCapabilities.config.backend !== 'rust') return Promise.resolve(false);
  const key = viewKey(uid, agent);
  const current = migrationReadProbes.get(key);
  if (current) return current;
  const task = (async () => {
    const ac = new AbortController();
    const timer = setTimeout(() => ac.abort(), SYNC_STALL_MS);
    try {
      const entry = cache.get(key);
      await fetchMessages(uid, {agent, start: entry?.end, head: entry?.version?.head,
        anchor: entry?.anchor, signal: ac.signal});
      return false;
    } catch (reason) {
      return !!reportReadFailure(uid, agent, reason);
    } finally { clearTimeout(timer); }
  })().finally(() => {
    if (migrationReadProbes.get(key) === task) migrationReadProbes.delete(key);
  });
  migrationReadProbes.set(key, task);
  return task;
}

function syncSession(uid, agent = S.agent) {
  const key = viewKey(uid, agent);
  if (migrationReadPaused(uid, agent)) return Promise.resolve(0);
  const e = cache.get(key);
  if (!e) return Promise.resolve(0);
  const current = syncingViews.get(key);
  if (current) return current;
  const task = (async () => {
    const ac = new AbortController();
    let stallTimer = null;
    const keepAlive = () => {
      clearTimeout(stallTimer);
      stallTimer = setTimeout(() => ac.abort(), SYNC_STALL_MS);
    };
    try {
      keepAlive();
      const { data, bytes } = await fetchMessages(uid, {
        agent, start: e.end, head: e.version.head, anchor: e.anchor,
        signal: ac.signal, onActivity: keepAlive });
      return await applyDiff(uid, data, bytes, agent);
    } catch (error) {
      // 瞬时失败不登记：tickSync 会按自适应间隔再来，SSE 也照常重连。
      reportReadFailure(uid, agent, error);
      return 0;
    } finally {
      clearTimeout(stallTimer);
    }
  })().finally(() => {
    if (syncingViews.get(key) === task) syncingViews.delete(key);
  });
  syncingViews.set(key, task);
  return task;
}

// ---- 服务端推送 ----
// 服务端盯着会话文件, 一变就把 diff 推过来, 不用客户端反复问。
let _es = null, _esUid = null, _esRetry = null;
let _esRetryAttempt = 0;   // 连续未成功打开的次数，决定重连退避
const diffRecoveries = new Map();

/** 游标冲突或队列先于正文确认时，从当前已接受游标重新取一次并重建 watch。 */
function scheduleDiffRecovery(uid, agent = null) {
  const key = viewKey(uid, agent);
  if (diffRecoveries.has(key)) return;
  const timer = setTimeout(async () => {
    try {
      // 若冲突发生时已有主动拉取在途，先等旧请求收尾，再保证至少发出
      // 一次基于最新本地游标的新请求。
      const inFlight = syncingViews.get(key);
      if (inFlight) await inFlight;
      if (!cache.has(key)) return;
      await syncSession(uid, agent);
      if (S.sel === uid && S.agent === agent) watchSession(uid, agent);
    } finally {
      if (diffRecoveries.get(key) === timer) diffRecoveries.delete(key);
    }
  }, 0);
  diffRecoveries.set(key, timer);
}

function watchSession(uid, agent = S.agent) {
  closeWatch();
  if (SessionDockNetwork.paused) return;
  if (migrationReadPaused(uid, agent)) { renderMigrationReadFailure(uid, agent); return; }
  const e = cache.get(viewKey(uid, agent));
  if (!e || !window.EventSource) return;
  // watch 从当前缓存游标开始，建立过程中不需要 tickSync 立刻再发一条相同
  // 增量请求。否则 CONNECTING 尚未变 OPEN 的几百毫秒会产生一次竞争包。
  if (S.sel === uid && S.agent === agent) S.lastSync = Date.now();
  const connectionId = globalThis.crypto?.randomUUID?.()
    || `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const p = new URLSearchParams({ uid, start: e.end, head: e.version.head,
    anchor: e.anchor || '', page: AUDIT_PAGE_ID, connection: connectionId });
  if (agent) p.set('agent', agent);
  const es = new EventSource(appUrl('api/watch?' + p));
  _es = es;
  _esUid = uid;
  es.__sessiondockConnectionId = connectionId;
  let received = 0;
  browserAuditEvent('sse.connecting', {start: e.end, agent: agent || ''}, null,
    {uid, connectionId});
  let opened = false;
  es.onopen = () => {
    opened = true;
    if (_es === es) _esRetryAttempt = 0;
    browserAuditEvent('sse.opened', {ready_state: es.readyState}, null, {uid, connectionId});
  };
  if (SessionDockCapabilities.config.backend === 'rust') {
    es.addEventListener('migration-error', event => {
      let reason;
      try { reason = JSON.parse(event.data); }
      catch { reason = {error: '实时同步返回了无效的错误信息，请重试。'}; }
      pauseMigrationWatch(es, uid, agent, reason);
    });
  }
  es.onmessage = ev => {
    // close() 后浏览器仍可能派发已经排队的旧事件，不能让旧 watch 改新视图。
    if (_es !== es || _esUid !== uid || S.agent !== agent) return;
    let data;
    try { data = JSON.parse(ev.data); }
    catch (error) {
      browserAuditEvent('sse.parse_failed', {error: String(error), bytes: ev.data.length},
        ev.data.slice(0, 4000), {uid, connectionId, severity: 'error'});
      return;
    }
    received++;
    const packetId = data._audit?.packet_id || `${connectionId}:client-${received}`;
    browserAuditEvent('sse.received', {
      packet_id: packetId, kind: data._audit?.kind || '', bytes: ev.data.length,
      reset: !!data.reset, start: data.start, end: data.end,
      messages: data.messages?.length || 0, outbox: data.outbox?.length || 0,
    }, null, {uid, traceId: packetId, connectionId});
    Promise.resolve(applyDiff(uid, data, 0, agent)).then(applied => {
      const snapshot = browserStateSnapshot('sse-applied');
      browserAuditEvent('sse.applied', {
        packet_id: packetId, applied_messages: applied,
        cache_end: cache.get(viewKey(uid, agent))?.end,
      }, snapshot.content, {uid, traceId: packetId, connectionId});
      scheduleBrowserSnapshot('sse-applied');
    }).catch(error => browserAuditEvent('sse.apply_failed', {
      packet_id: packetId, error: String(error?.stack || error),
    }, null, {uid, traceId: packetId, connectionId, severity: 'error'}));
  };
  es.onerror = () => {
    // EventSource 自带的重连会沿用旧 URL(旧偏移), 所以自己关掉重开, 带上新偏移
    es.close();
    browserAuditEvent('sse.error', {ready_state: es.readyState, received, opened}, null,
      {uid, connectionId, severity: 'warning'});
    if (_es !== es) return;
    _es = null;
    clearTimeout(_esRetry);
    if (SessionDockCapabilities.config.backend !== 'rust') {
      _esRetry = setTimeout(() => {
        if (S.sel === uid && S.agent === agent) watchSession(uid, agent);
      }, 1500);
      return;
    }
    // Rust：断网、代理断开、服务重启、503 都是瞬时的——按 1.5 s 起的指数退避
    // 重开（封顶 15 s），不暂停视图。流从未打开过时先探一次 HTTP 原因，
    // 只有探到不可恢复的失败（如 501）才暂停。
    const attempt = opened ? 0 : _esRetryAttempt++;
    const delay = retryDelay(attempt);
    const reopen = () => {
      if (S.sel !== uid || S.agent !== agent || _es || migrationReadPaused(uid, agent)) return;
      watchSession(uid, agent);
    };
    const schedule = () => { clearTimeout(_esRetry); _esRetry = setTimeout(reopen, delay); };
    if (opened) schedule();
    else probeWatchRejection(uid, agent).then(paused => { if (!paused) schedule(); });
  };
}

function closeWatch() {
  clearTimeout(_esRetry);
  if (_es) {
    browserAuditEvent('sse.closed_by_page', {ready_state: _es.readyState}, null,
      {uid: _esUid, connectionId: _es.__sessiondockConnectionId || ''});
    _es.close(); _es = null; _esUid = null;
  }
}

function unreadRow(uid) {
  const value = S.unread.get(uid);
  if (typeof value === 'number') return { count: value };
  return value && typeof value === 'object'
    ? { count: Math.max(0, +value.count || 0) }    // 旧记录里的 tmux 标志不再使用
    : { count: 0 };
}

function saveUnread() {
  store.set('unread', [...S.unread].filter(([, row]) => (+row?.count || +row || 0) > 0));
}

function sessionFrozen(uid) {
  return !!uid && typeof T !== 'undefined' && (T.list || []).some(row => !row.stale
    && row.frozen === true && (row.uid === uid || row.current_uid === uid || `tmux:${row.name}` === uid));
}

function paintStatusMarker(badge, frozen, count = 0, attention = '') {
  const question = !frozen && attention === 'question';
  badge.classList.toggle('frozen', frozen);
  badge.classList.toggle('input-attention', !frozen && !!attention);
  badge.classList.toggle('input-question', question);
  const marker = `${frozen}:${count}:${attention}`;
  if (badge.dataset.marker === marker) return;
  badge.dataset.marker = marker;
  const text = count > 99 ? '99+' : (count || '');
  if (frozen) badge.innerHTML = uiIcon('pause');
  else badge.textContent = question ? '?' : text;
}

function sessionInputAttention(uid) {
  if (!uid || (typeof sessionComposerEnded === 'function' && sessionComposerEnded(uid))) return '';
  const draft = typeof composerDrafts !== 'undefined'
    ? composerDrafts.get(composerDraftOwner(uid)) : null;
  const cli = cache.get(uid)?.cli;
  const current = typeof composerDrafts !== 'undefined'
    && composerDraftOwner(uid) === composerDraftOwner(composerUid);
  const input = current ? draft?.inputStatus || cli?.input : cli?.input;
  // Normal startup/screen synchronization/paste is not a request for help.
  if (input?.state === 'starting' || ['input_check_pending', 'cli_starting',
      'cli_catching_up', 'cli_pasting'].includes(input?.code)) return '';
  const turn = sessionTurn(uid);
  if (input?.code === 'cli_question' || turn === 'waiting') return 'question';
  return '';
}
const inputAttentionLabel = attention => attention === 'question' ? ' · 等待回答' : '';

/** 角标颜色只说现在：绿 = 在跑，蓝 = 在跑且在受管终端里，灰 = 已退出但还有没看的新内容。
 *  颜色不随计数固化——以前把计数时的 tmux 态存进 localStorage，会话退出后角标还是蓝的。 */
function paintItemStatus(node) {if (node) refreshSidebarRows(node.dataset.uid);}

/** 回合状态只在进程还在跑时区分：working = 正在轮转，waiting = 等你回答，idle = 停在输入框。
 *  正在看的会话用最新的对话 activity 和 CLI 画面（cli.instance.busy，docs/cli-state.md），
 *  其他行用列表的 turn（docs/read-model.md）。缺字段的旧节点返回空串，只显示运行点。 */
function sessionTurn(uid) {
  if (!uid || !S.live.has(uid)) return '';
  const entry = S.sel === uid ? cache.get(viewKey(uid)) : null;
  const row = indexedSessions().byUid.get(uid);
  // 列表 turn 与左栏同一规则；对话 activity 只补上更早到达的"等待回答"。
  let state = entry?.activity?.state === 'waiting' ? 'waiting' : row?.turn || '';
  const busy = entry?.cli?.instance?.busy;
  if (state !== 'waiting' && typeof busy === 'boolean') state = busy ? 'working' : 'idle';
  // 主回合结束但后台子代理或后台任务（Monitor、后台命令）还在跑：会话在等它们，仍算轮转中。
  if (state !== 'waiting' && (S.liveWorking.has(uid) || row?.background > 0 || (row?.agent_items || []).some(item => agentRunning(uid, item)))) state = 'working';
  return ['working', 'waiting'].includes(state) ? state : (state ? 'idle' : '');
}
const turnLabel = turn => ({working: ' · 正在处理', waiting: ' · 等待回答', idle: ' · 空闲'})[turn] || '';

function paintTurn(uid) {
  paintItemStatus(document.querySelector(`.item[data-uid="${CSS.escape(uid)}"]`));
  if (uid === S.sel) paintHeaderTurn();
}

function paintHeaderTurn() {
  syncSessionFreezeOverlay();
  const h = $('#dlive');
  if (!h) return;
  const row = document.querySelector(`.item[data-uid="${CSS.escape(S.sel || '')}"]`);
  const tmux = row ? row.classList.contains('live-tmux') : S.liveTmux.has(S.sel);
  const frozen = sessionFrozen(S.sel);
  const draft = typeof composerDrafts !== 'undefined'
    ? composerDrafts.get(composerDraftOwner(S.sel)) : null;
  const active = (row ? row.classList.contains('live') : S.live.has(S.sel))
    || (draft?.cli?.instance?.running === true && !!takenOver(S.sel));
  const attention = !frozen && active ? sessionInputAttention(S.sel) : '';
  paintStatusMarker(h, frozen, 0, attention);
  h.classList.toggle('visible', frozen || active);
  const turn = frozen || row?.dataset.tmuxName ? '' : sessionTurn(S.sel);
  h.classList.toggle('turn-working', turn === 'working' && !attention);
  h.classList.toggle('turn-waiting', turn === 'waiting');
  h.title = h.ariaLabel = frozen ? '会话已暂停' : liveStatusTitle(tmux)
    + turnLabel(turn) + (attention && turn !== 'waiting' ? inputAttentionLabel(attention) : '');
}

function addUnread(uid, count) {
  if (!uid || count <= 0) return;
  const row = unreadRow(uid);
  S.unread.set(uid, { count: row.count + count });
  saveUnread();
  paintItemStatus(document.querySelector(`.item[data-uid="${CSS.escape(uid)}"]`));
}

function clearUnread(uid) {
  if (!uid || !S.unread.has(uid)) return;
  S.unread.delete(uid);
  saveUnread();
  paintItemStatus(document.querySelector(`.item[data-uid="${CSS.escape(uid)}"]`));
}

/** 兜底轮询: SSE 连着的时候只是很慢地对一下账, 断了才回到自适应的快节奏。 */
function tickSync() {
  if (SessionDockNetwork.paused) return;
  if (!S.sel || document.hidden) return;
  if (migrationReadPaused(S.sel, S.agent)) return;
  const pushing = _es && _esUid === S.sel && _es.readyState === 1;
  const gap = pushing ? BACKUP_MS : (S.live.has(S.sel) ? S.syncGap : SYNC_MS);
  if (Date.now() - (S.lastSync || 0) < gap) return;
  S.lastSync = Date.now();
  const uid = S.sel;
  const agent = S.agent;
  syncSession(uid, agent).then(n => {
    if (uid !== S.sel || agent !== S.agent || pushing) return;
    S.syncGap = n ? FAST_MIN : Math.min(FAST_MAX, Math.round(S.syncGap * 1.5));
  });
}

setInterval(tickSync, TICK_MS);

// ---- 活跃会话 ----
async function refreshLive(force = false) {
  if (!SessionDockCapabilities.allows('live')) return;
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 12000);
  let d;
  try {
    const response = await fetch(appUrl('api/live' + (force ? '?force=1' : '')), {signal:controller.signal});
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    d = await response.json();
  } finally { clearTimeout(timeout); }
  if (!Array.isArray(d.uids)) throw new Error('运行状态格式错误');
  applyNodeState(d, 'live');
  const next = new Set(d.uids);
  const nextTmux = new Set((d.tmux_uids || []).filter(u => next.has(u)));
  const nextWorking = new Set((d.working_uids || []).filter(u => next.has(u)));
  // Work another machine runs for a session, named by native node/source/sid.
  const remoteKey = (node, source, sid) => JSON.stringify([node || '', source, String(sid)]);
  const remote = new Set((Array.isArray(d.remote_working) ? d.remote_working : [])
    .map(r => remoteKey(r.node_id, r.source, r.sid)));
  if (remote.size) for (const s of S.sessions || []) {
    if (next.has(s.uid) && remote.has(remoteKey(s.node_id, s.source, s.sid))) nextWorking.add(s.uid);
  }
  const nextStarted = new Map(Object.entries(d.started_at || {}).map(([u, t]) => [u, +t]));
  const setChanged = (a, b) => a.size !== b.size || [...a].some(u => !b.has(u));
  const mapChanged = (a, b) => a.size !== b.size
    || [...a].some(([u, t]) => b.get(u) !== t);
  const changed = setChanged(next, S.live) || setChanged(nextTmux, S.liveTmux)
    || setChanged(nextWorking, S.liveWorking)
    || mapChanged(nextStarted, S.liveStarted);
  S.live = next;
  S.liveTmux = nextTmux;
  S.liveWorking = nextWorking;
  S.liveStarted = nextStarted;
  trimCache();
  if (changed) {
    paintLive();
  }
}

let livePollRequest = null;
function pollLive(force = false) {
  if (SessionDockNetwork.paused) return Promise.resolve();
  if (!SessionDockCapabilities.allows('live')) return Promise.resolve();
  if (document.hidden) return Promise.resolve();
  if (livePollRequest) {
    // Timer ticks share the current cycle; explicit refreshes still get a
    // fresh snapshot after it, and awaiters never observe a half-finished poll.
    return force ? livePollRequest.then(() => pollLive(true)) : livePollRequest;
  }
  livePollRequest = runLivePoll(force).finally(() => { livePollRequest = null; });
  return livePollRequest;
}

async function runLivePoll(force) {
  try {
    await refreshLive(force);
    if (typeof loadTermList === 'function') {   // tmux 会话可能在外部被结束
      // 首屏：term.js 自己已经发出了列表请求，复用它；列表从"未加载"变为有内容
      // 不算变化，否则每次打开页面都会多一轮强制 live 刷新。刚返回不到 1 秒的
      // 列表也不重拉（首屏 live 与 term.js 的初始请求前后脚到达）；显式强刷除外。
      const first = !T.listLoaded;
      const before = (T.list || []).map(x => x.name).join();
      const fresh = !force && T.listLoadedAt && performance.now() - T.listLoadedAt < 1000;
      if (first && T.listRequest) await T.listRequest;
      else if (!fresh) await loadTermList();
      if (!first && (T.list || []).map(x => x.name).join() !== before) {
        await refreshLive(true);                // 绕过 3 秒缓存，绿点立即跟着 tmux 消失
        renderTakeoverBtn();
        if (T.name && !T.list.some(x => x.name === T.name)) closeTermPane();
      }
    }
  } catch { /* 服务端没起来就下轮再说 */ }
}

/** 只改小圆点, 不重渲染整个列表 —— 否则每几秒就会打断滚动和选中。 */
function paintLive() {
  for (const group of SessionDockSidebar.currentGroups()) for (const view of group.rows) {
    const session = view.row.s;
    if (session.tmuxName) pendingSidebarLive.set(session,
      typeof T !== 'undefined' && !!T.list?.some(row => row.name === session.tmuxName && !row.stale));
  }
  refreshSidebarRows();
  const h = $('#dlive');
  if (h) {
    // 临时会话（还没有 JSONL）按左栏同样的规则算运行中
    const row = document.querySelector(`.item[data-uid="${CSS.escape(S.sel || '')}"]`);
    const live = row ? row.classList.contains('live') : S.live.has(S.sel);
    const tmux = row ? row.classList.contains('live-tmux') : S.liveTmux.has(S.sel);
    h.classList.toggle('visible', live);
    h.classList.toggle('tmux', tmux);
    paintHeaderTurn();
  }
  const selected = S.sessions.find(x => x.uid === S.sel);
  if (selected) {
    renderSessionAction(selected);
    renderConversationTail(cache.get(viewKey(selected.uid, S.agent))?.activity, selected.uid);
  }
  paintTransferAvailability($('#a-clone-group'), S.sel);
  if (menuUid) paintTransferAvailability($('#item-menu [data-act="clone"]'), menuUid);
  renderSessionCounts();
  syncActiveOnlyList();
  if (S.picking) renderPickBar();
}

/** 活动筛选开启时，进程启停会改变列表成员；集合没变就不动 DOM。 */
function syncActiveOnlyList() {
  if (!S.activeOnly) return;
  const wanted = new Set(visible().map(s => s.uid));
  const shown = $('#side')?._sessionUids || new Set();
  if (wanted.size === shown.size && [...wanted].every(uid => shown.has(uid))) return;
  const side = $('#side'), top = side?.scrollTop || 0;
  renderSide();
  if (side) side.scrollTop = top;
}

function selectSessionScope(activeOnly) {
  if (activeOnly && !SessionDockCapabilities.allows('live')) {
    showConsoleToast('运行状态未知：Rust 后端尚未实现进程探测，不能按活跃状态筛选。');
    return;
  }
  S.activeOnly = activeOnly;
  store.set('activeOnly', S.activeOnly);
  renderSide();
  paintLive();
}
function sidebarScopeKeydown(e) {
  if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'].includes(e.key)) return;
  e.preventDefault();
  const active = e.key === 'Home' ? true : e.key === 'End' ? false : !S.activeOnly;
  selectSessionScope(active);
  $(active ? '#livecount' : '#allcount').focus();
}


setInterval(() => { if (!uiEventsReady) pollLive(); }, LIVE_MS);
document.addEventListener('visibilitychange', () => {
  if (document.hidden) { closeWatch(); return; }
  pollLive();
  if (S.sel && cache.get(viewKey(S.sel, S.agent))) {
    syncSession(S.sel, S.agent).then(() => watchSession(S.sel, S.agent));
  }
});

// ---------------------------------------------------------------- 数据加载
function renderSessionCounts() {
  ensureSidebarVue();
  const pool = sidebarSessions().filter(s => !S.off.has(s.source) && nodeSelected(s));
  const active = pool.filter(s => s.pending || S.live.has(s.uid)).length;
  SessionDockSidebar.updateCounts(active, pool.length, SessionDockCapabilities.allows('live'), S.activeOnly, selectSessionScope, sidebarScopeKeydown);
}

function showSessionCount() {
  renderSessionCounts();
  SessionDockSearch.status.textContent = '';
}

const pendingUid = name => `tmux:${name}`;

// A draft saved before `session.started` existed has no start time. Pin the
// moment this page first listed it so the row does not move on every render.
const pendingDraftFirstSeen = new Map();
function pendingDraftStartedAt(uid, session) {
  const started = Number(session?.started);
  if (Number.isFinite(started) && started > 0) return started;
  if (!pendingDraftFirstSeen.has(uid)) pendingDraftFirstSeen.set(uid, Date.now() / 1000);
  return pendingDraftFirstSeen.get(uid);
}

/** SessionDock启动、但还没有对话文件的 tmux，也是一条可重新进入的临时会话。 */
function pendingTmuxSessions() {
  if (typeof T === 'undefined' || !Array.isArray(T.pending)) return [];
  const pending = [...T.pending];
  // A CLI may exit before creating native history (for example after updating).
  // Keep its saved input reachable instead of removing the only recovery entry.
  if (typeof composerDrafts !== 'undefined') {
    const names = new Set(pending.map(row => row.name));
    for (const [uid, draft] of composerDrafts) {
      if (!uid.startsWith('tmux:') || !draft.session || names.has(draft.session.name)
          || (!draft.text && !draft.attachments.length && !draft.quotes.length)) continue;
      pending.push({ ...draft.session, stale: true, running: false,
        state: terminalListUncertain(uid) ? 'uncertain' : 'exited',
        started: pendingDraftStartedAt(uid, draft.session), unavailable_reason: '会话草稿已保留' });
    }
  }
  return pending.flatMap(t => {
    // A receipt whose binding the server confirmed is represented by
    // the native row it binds, exactly like a declared Claude identity.
    const native = pendingNativeKey(t);
    if (!SOURCES[t.source] || (native && indexedSessions().byNative.has(native))) return [];
    const source = t.source;
    return [{
      node_id: t.node_id, node_name: t.node_name, stale: t.stale,
      uid: pendingUid(t.name), pending: true, name: t.name, tmuxName: t.name, source,
      ...(SessionDockCapabilities.config.backend === 'rust' ? {record_id:t.record_id,launch_id:t.launch_id,
        instance_id:t.instance_id,running:t.running,state:t.state,unavailable_reason:t.unavailable_reason,
        native_binding:t.native_binding,binding:t.binding,recording:t.recording,grid:t.grid} : {}),
      title: t.title || `新建 ${SOURCES[source].name} 会话`,
      kind: t.kind || '', report_id: t.report_id || '', cwd: t.cwd || '(未知)',
      created: new Date(pendingDraftStartedAt(pendingUid(t.name), t) * 1000).toISOString(),
      updated: new Date(pendingDraftStartedAt(pendingUid(t.name), t) * 1000).toISOString(),
      size: 0,
    }];
  });
}

function pendingNativeKey(t) {
  const declared = t.sid || t.declared_sid || (t.binding?.state === 'confirmed' ? t.binding.sid : '');
  return declared ? JSON.stringify([t.node_id || '', t.source, String(declared)]) : '';
}

// Deleting native rows also discards the launch receipts they hid on the
// server. Drop those receipts here too: otherwise the stale T.pending brings
// the session back as a pending row until the next terminal list.
function forgetDeletedReceipts(rows) {
  if (typeof T === 'undefined' || !Array.isArray(T.pending)) return;
  const keys = new Set(rows.filter(row => row.sid)
    .map(row => JSON.stringify([row.node_id || '', row.source, String(row.sid)])));
  if (keys.size) T.pending = T.pending.filter(t => !keys.has(pendingNativeKey(t)));
}

let sessionIndexRows = null, sessionIndex = null;
function indexedSessions() {
  if (sessionIndexRows !== S.sessions) {
    sessionIndexRows = S.sessions;
    const byUid = new Map(), byNative = new Map(), forkChildren = new Map();
    for (const row of S.sessions) {
      byUid.set(row.uid, row);
      const key = JSON.stringify([row.node_id || '', row.source, String(row.sid)]);
      if (!byNative.has(key)) byNative.set(key, row);
      if (row.sid && row.forked_from_id) {
        const parent = JSON.stringify([row.node_id || '', row.source, String(row.forked_from_id)]);
        if (!forkChildren.has(parent)) forkChildren.set(parent, []);
        forkChildren.get(parent).push(row);
      }
    }
    sessionIndex = {byUid, byNative, forkChildren};
  }
  return sessionIndex;
}
const sessionContinued = session =>
  !!(session?.continued_in && indexedSessions().byUid.has(session.continued_in));
const hiddenForkParent = session => !!session?.fork_parent && !session.fork_parent_visible;
const sessionHidden = session => hiddenForkParent(session) || sessionContinued(session);
// 沿 forked_from_id 往上追整条父会话链（近的在前）。只在同来源、同机器内按
// 原生 sid 匹配；记录已不存在的一级保留占位并到此为止。
function forkAncestors(session) {
  const chain = [];
  const seen = new Set();
  let sid = String(session?.forked_from_id || '');
  while (sid && !seen.has(sid)) {
    seen.add(sid);
    const row = indexedSessions().byNative.get(JSON.stringify([session.node_id || '', session.source, sid]));
    chain.push({ sid, row: row || null });
    sid = String(row?.forked_from_id || '');
  }
  return chain;
}
// 沿 forked_from_id 往下追到最深的回退分支：Codex 双 Esc 后进程不变，新消息只写
// 进新分支的文件。同一级有多条分支时先取仍在运行的，再取最新创建的。
// 一条会话的直接子分支（同来源、同机器，forked_from_id 指向它）：仍在运行的
// 在前，其余按创建时间新的在前。
function forkChildren(session) {
  if (!session?.sid) return [];
  return [...(indexedSessions().forkChildren.get(JSON.stringify([session.node_id || '', session.source, String(session.sid)])) || [])]
    .sort((a, b) => (S.live.has(b.uid) - S.live.has(a.uid))
      || String(b.created || '').localeCompare(String(a.created || '')));
}
function forkLeaf(session) {
  const seen = new Set();
  let cur = session;
  while (cur && !seen.has(cur.uid)) {
    seen.add(cur.uid);
    const children = forkChildren(cur);
    if (!children.length) break;
    cur = children[0];
  }
  return cur;
}
function forkLeafUid(uid) {
  return forkLeaf(S.sessions.find(s => s.uid === uid))?.uid || uid;
}
const sidebarSessions = () => [...pendingTmuxSessions(), ...S.sessions]
  .filter(s => !sessionHidden(s));

function cursorViews(sessions) {
  const rows = [];
  for (const session of sessions) {
    if (session.cursor) rows.push({uid: session.uid, agent: null, cursor: session.cursor});
    for (const item of session.agent_items || []) {
      if (item.cursor) rows.push({uid: session.uid, agent: item.id, cursor: item.cursor});
    }
  }
  return rows;
}

function cleanCursor(value) {
  return value && Number.isFinite(+value.end) && value.head
    ? {end: +value.end, head: String(value.head), anchor: String(value.anchor || '')}
    : null;
}

function seedSidebarCursors(sessions) {
  for (const row of cursorViews(sessions)) {
    const cursor = cleanCursor(row.cursor);
    if (cursor) S.cursors.set(viewKey(row.uid, row.agent), cursor);
  }
}

const sidebarSyncing = new Set(), sidebarPendingCursors = new Map();

// Background views only need unread counts, even when their old history is cached.
// Message bodies are refreshed on selection. Explicit node routes carry local UIDs.
const unreadBatches = new Map(), unreadBatchUnsupported = new Set();
function fetchUnreadSummary(uid, opts) {
  const node = nodeOf(uid);
  if (!SessionDockCapabilities.config.unread_batch || unreadBatchUnsupported.has(node))
    return Promise.resolve({data: {unsupported: true}});
  return new Promise((resolve, reject) => {
    let batch = unreadBatches.get(node);
    if (!batch) {
      batch = [];
      unreadBatches.set(node, batch);
      setTimeout(() => flushUnreadBatch(node, batch), 0);
    }
    batch.push({uid, opts, resolve, reject});
  });
}
async function flushUnreadBatch(node, batch) {
  unreadBatches.delete(node);
  const path = node ? `api/nodes/${node}/api/sessions/unread` : 'api/sessions/unread';
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), SYNC_STALL_MS);
  try {
    const response = await fetch(appUrl(path), {method: 'POST', signal: controller.signal,
      headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({views: batch.map(({uid, opts}) => ({
        uid: node ? uid.replace(`:${node}~`, ':') : uid, agent: opts.agent || '',
        start: opts.start || 0, head: opts.head || '', anchor: opts.anchor || '',
      }))})});
    if ([404, 405, 501].includes(response.status)) {
      await response.arrayBuffer();
      unreadBatchUnsupported.add(node); // Old nodes during a rolling upgrade.
      for (const item of batch) item.resolve({data: {unsupported: true}});
      return;
    }
    const payload = await response.json();
    if (!response.ok || !Array.isArray(payload.results) || payload.results.length !== batch.length) {
      const error = new Error(payload.error || 'Invalid unread summary response');
      error.status = response.ok ? 502 : response.status;
      throw error;
    }
    payload.results.forEach((data, i) => {
      if (data.error) {
        const error = new Error(data.error); error.status = data.status;
        batch[i].reject(error);
      } else batch[i].resolve({data, bytes: 0});
    });
  } catch (error) {
    for (const item of batch) item.reject(error);
  } finally { clearTimeout(timer); }
}

async function syncSidebarView(row, base, latest, attempt = 0) {
  const key = viewKey(row.uid, row.agent);
  if (sidebarSyncing.has(key)) {
    sidebarPendingCursors.set(key, row.agent
      ? {uid: row.uid, agent_items: [{id: row.agent, cursor: latest}]}
      : {uid: row.uid, cursor: latest});
    return;
  }
  sidebarSyncing.add(key);
  try {
    const sameCheckpoint = () => {
      const current = cleanCursor(S.cursors.get(key));
      return current && current.end === base.end && current.head === base.head
        && current.anchor === base.anchor;
    };
    if (!sameCheckpoint()) return;
    const {data} = await fetchUnreadSummary(row.uid, {
      agent: row.agent, start: base.end, head: base.head, anchor: base.anchor,
      appendOnly: true,
    });
    // Selection or another accepted update owns the newer checkpoint. A delayed
    // summary must neither reintroduce unread badges nor rewind that checkpoint.
    if (!sameCheckpoint() || (S.sel === row.uid && S.agent === row.agent
        && (!MOBILE.matches || document.body.classList.contains('mobile-detail')))) return;
    if (data.unsupported) {
      // Old nodes cannot give an exact count without downloading history. Show
      // at least one unread item and defer the actual content until selection.
      if (!unreadRow(row.uid).count) addUnread(row.uid, 1);
      S.cursors.set(key, latest);
      return;
    }
    if (data.reset) cache.delete(key);
    else if (data.incoming) addUnread(row.uid, data.incoming);
    S.cursors.set(key, cleanCursor({end: data.end, head: data.version.head,
                                   anchor: data.anchor}) || latest);
  } catch (error) {
    // 列表签名可能不会再变化；瞬时失败（断网、503）按退避补三次（1.5/3/6 s），
    // 仍不影响其他会话；4xx 这类请求本身不成立的失败不重试。
    if (attempt < 3 && (SessionDockCapabilities.config.backend !== 'rust' || transientReadFailure(error))) {
      setTimeout(() => syncSidebarView(row, base, latest, attempt + 1), retryDelay(attempt));
    }
  }
  finally {
    sidebarSyncing.delete(key);
    const pending = sidebarPendingCursors.get(key);
    sidebarPendingCursors.delete(key);
    if (pending) syncSidebarUpdates([pending]);
  }
}

/** 列表发现其他会话文件增长时，只读取追加区间并累加左栏未读数。 */
function syncSidebarUpdates(sessions) {
  for (const row of cursorViews(sessions)) {
    const key = viewKey(row.uid, row.agent);
    const latest = cleanCursor(row.cursor);
    if (!latest) continue;
    const entry = cache.get(key);
    const base = cleanCursor(S.cursors.get(key)) || (entry
      ? cleanCursor({end: entry.end, head: entry.version?.head, anchor: entry.anchor})
      : null);
    if (!base) { S.cursors.set(key, latest); continue; }
    if (!S.cursors.has(key)) S.cursors.set(key, base);
    // Rust 列表行的 cursor 只带物理部分 {end, head}；语义 anchor 只在该会话已被
    // 打开（服务端缓存有视图）时出现。两边都有 anchor 才比较它，缺失时以
    // 已有的 anchor 为准（docs/history-pages.md "列表 cursor"）。
    const anchorSame = !base.anchor || !latest.anchor || base.anchor === latest.anchor;
    if (base.end === latest.end && base.head === latest.head && anchorSame) {
      S.cursors.set(key, latest.anchor ? latest : {...latest, anchor: base.anchor});
      continue;
    }
    const detailVisible = S.sel === row.uid && S.agent === row.agent
      && (!MOBILE.matches || document.body.classList.contains('mobile-detail'));
    if (detailVisible) {                     // 当前正在看的新增内容直接视为已读
      S.cursors.set(key, latest);
      continue;
    }
    if (latest.end < base.end) {
      cache.delete(key);                      // 明确回滚，不尝试整份后台下载
      S.cursors.set(key, latest);
      continue;
    }
    void syncSidebarView(row, base, latest);
  }
}

function mergeSessionMetaEvent(entry, session) {
  if (entry.meta.agent_id || !session.renamed_at || !session.renamed_to) return false;
  const eventId = `rename:${session.sid}:${session.renamed_at}`;
  if (entry.msgs.some(m => m.event_id === eventId)) return false;
  const event = {
    role: 'command', text: `/rename ${session.renamed_to}`, ts: session.renamed_at,
    counted: false, inferred: true, event_id: eventId,
  };
  const at = entry.msgs.findIndex(m => m.ts && m.ts > event.ts);
  entry.msgs.splice(at < 0 ? entry.msgs.length : at, 0, event);
  messageIndexes.delete(entry.msgs);
  return true;
}

/** 列表元数据变更后同步缓存和当前详情标题，不重绘消息正文。 */
function refreshSessionMeta() {
  const headerKey = m => JSON.stringify([
    m.title, m.parent_title, m.sid, m.agent_type, m.model, !!m.starred,
    m.nest_parent || null, m.group || null,
    (m.agent_items || []).map(a => [a.id, a.title, a.type]),
  ]);
  const before = cache.get(viewKey(S.sel, S.agent));
  const beforeKey = before ? headerKey(before.meta) : '';
  let currentEventAdded = false;
  for (const e of cache.values()) {
    const s = indexedSessions().byUid.get(e.meta.uid);
    if (s) {
      const agent = e.meta.agent_id;
      if (!agent) {
        e.meta = { ...e.meta, ...s };
        if (mergeSessionMetaEvent(e, s) && e === before) currentEventAdded = true;
        continue;
      }
      const item = (s.agent_items || []).find(a => a.id === agent);
      if (!item) continue;
      const childPath = e.meta.path;
      e.meta = {
        ...e.meta, ...s, path: childPath,
        cwd: item.cwd ?? e.meta.cwd, model: item.model ?? e.meta.model,
        created: item.created ?? e.meta.created,
        sid: agent, title: item.title, size: item.size, updated: item.updated,
        agent_id: agent, agent_type: item.type, parent_title: s.title,
      };
    }
  }
  const current = cache.get(viewKey(S.sel, S.agent));
  const oldHead = $('#detail > .dhead');
  if (currentEventAdded && current) {
    renderSession(current.meta, current.msgs, current.activity);
  } else if (current && oldHead && headerKey(current.meta) !== beforeKey) {
    oldHead.replaceWith(head(current.meta, entryTotal(current)));
    layoutSessionHead();
    auditDetailRendered('meta-refresh');
  }
}

let sessionLoadRun = 0;
let sessionLoadRetry = null;
let sessionPollRequest = null, sessionPollController = null, sessionLoadActive = 0;

async function loadSessions(force) {
  if (SessionDockNetwork.paused) return false;
  const run = ++sessionLoadRun;
  sessionLoadActive = run;
  sessionPollController?.abort();
  clearTimeout(sessionLoadRetry);
  SessionDockSearch.status.textContent = force ? ' 重新扫描…' : ' 加载中…';
  const ac = new AbortController();
  const timeout = setTimeout(() => ac.abort(), 15000);
  let d;
  try {
    const r = await fetch(appUrl('api/sessions' + (force ? '?force=1' : '')), { signal: ac.signal });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    d = await r.json();
    if (!Array.isArray(d.sessions)) throw new Error('会话列表格式错误');
  } catch (e) {
    if (run !== sessionLoadRun || SessionDockNetwork.paused) return false;
    SessionDockSearch.status.textContent = ' 加载失败';
    SessionDockSearch.status.classList.add('err');
    ensureSidebarVue(); SessionDockSidebar.loadFailed(() => loadSessions(false));
    sessionLoadRetry = setTimeout(() => loadSessions(false), 3000);
    return false;
  } finally {
    clearTimeout(timeout);
    if (sessionLoadActive === run) sessionLoadActive = 0;
  }
  if (run !== sessionLoadRun) return false;
  SessionDockSearch.status.classList.remove('err');
  const seedCursors = S.cursors.size === 0;
  const wasListed = !hiddenForkParent(S.sessions.find(s => s.uid === S.sel));
  S.sig = d.sig;
  S.sessions = d.sessions;
  applyNodeState(d, 'sessions');
  refreshSessionMeta();
  renderChips();
  renderSide();
  showSessionCount(sidebarSessions().length);
  seedCursors ? seedSidebarCursors(d.sessions) : syncSidebarUpdates(d.sessions);
  if (wasListed) void followSelectedFork();
  return true;
}

/** 列表自动跟进磁盘变化。签名没变时服务端只回一个 unchanged, 成本为个位数毫秒。 */
function pollSessions() {
  if (SessionDockNetwork.paused) return Promise.resolve();
  if (document.hidden) return Promise.resolve();
  if (!S.sig || sessionLoadActive) return Promise.resolve(false);
  if (sessionPollRequest) return sessionPollRequest;
  sessionPollRequest = runSessionPoll().finally(() => { sessionPollRequest = null; });
  return sessionPollRequest;
}
async function runSessionPoll() {
  const run = sessionLoadRun, sig = S.sig;
  const ac = new AbortController();
  sessionPollController = ac;
  const timeout = setTimeout(() => ac.abort(), 15000);
  try {
    const response = await fetch(appUrl('api/sessions?sig=' + encodeURIComponent(sig)), {signal: ac.signal});
    if (!response.ok) return false;
    const d = await response.json();
    if (ac.signal.aborted || run !== sessionLoadRun || sig !== S.sig) return false;
    applyNodeState(d, 'sessions');
    if (d.unchanged || !d.sessions) return true;
    const wasListed = !hiddenForkParent(S.sessions.find(s => s.uid === S.sel));
    S.sig = d.sig;
    S.sessions = d.sessions;
    renderNodes();
    refreshSessionMeta();
    renderChips();
    syncSidebarUpdates(d.sessions);
    if (wasListed) await followSelectedFork();
    if (S.results) {
      // 搜索结果集合保持不变，只合入 rename 等最新元数据。
      const fresh = new Map(S.sessions.map(s => [s.uid, s]));
      S.results = S.results.map(r => {
        const latest = fresh.get(r.uid);
        if (!latest) return r;
        const agents = new Map((latest.agent_items || []).map(a => [a.id, a]));
        return {...r, ...latest, agent_items: (r.agent_items || []).map(a => ({
          ...a, ...agents.get(a.id), hits: a.hits, hits_capped: a.hits_capped, snippet: a.snippet,
        }))};
      });
      if (!patchSide(visible())) renderSide();
      return true;
    }
    showSessionCount(sidebarSessions().length);
    if (patchSide(visible())) return true;   // 能就地更新就不重建, 否则会一直闪
    const side = $('#side');
    const top = side.scrollTop;
    renderSide();                       // 选中态由 S.sel 恢复
    side.scrollTop = top;               // 别打断正在看的位置
    paintLive();
    return true;
  } catch { return false; }
  finally {
    clearTimeout(timeout);
    if (sessionPollController === ac) sessionPollController = null;
  }
}

setInterval(() => { if (!uiEventsReady) pollSessions(); }, LIST_MS);
document.addEventListener('visibilitychange', () => { if (!document.hidden) pollSessions(); });

// A single lightweight stream invalidates list state. Only the selected
// conversation has a separate body subscription; other views stay lazy.
let uiEvents = null, uiEventsReady = false, uiEventsRetry = 0;
let uiEventPending = null, uiEventApplying = false;
function queueUiChange(change) {
  if (!uiEventPending) uiEventPending = {live:false, term:false, sessions:false, cursors:new Map()};
  const pending = uiEventPending;
  for (const key of ['live', 'term', 'sessions']) pending[key] ||= !!(change.initial || change[key]);
  for (const row of change.cursors || []) pending.cursors.set(viewKey(row.uid, row.agent), row);
  void applyUiChanges();
}
async function applyUiChanges() {
  if (uiEventApplying) return;
  uiEventApplying = true;
  try {
    while (uiEventPending && !document.hidden) {
      const change = uiEventPending; uiEventPending = null;
      // An earlier poll may predate this invalidation. Wait for it, then
      // reconcile once more so its old response cannot consume the event.
      if (change.sessions && sessionPollRequest) await sessionPollRequest;
      const results = await Promise.allSettled([
        change.live ? refreshLive() : null,
        change.term && typeof loadTermList === 'function'
          ? loadTermList().then(() => !T.listError) : null,
        change.sessions ? pollSessions() : null,
      ]);
      if (results.some(result => result.status === 'rejected' || result.value === false)) {
        // A delivered event is not an acknowledgement of a successful read.
        // Reconnect obtains a new baseline even if nothing changes afterward.
        closeUiEvents();
        if (!document.hidden) uiEventsRetry = setTimeout(startUiEvents, 3000);
      }
      // A full list read already supplied newer cursors and synchronized
      // unread state. Do not overwrite it with the preceding event snapshot.
      if (change.sessions) continue;
      const rows = [];
      const sessions = new Map(S.sessions.map(row => [row.uid, row]));
      for (const update of change.cursors.values()) {
        const session = sessions.get(update.uid);
        if (!session) continue; // A later list invalidation introduces new rows.
        if (update.agent) {
          const agent = session.agent_items?.find(row => row.id === update.agent);
          if (agent) agent.cursor = update.cursor;
          rows.push({uid:update.uid, agent_items:[{id:update.agent,cursor:update.cursor}]});
        } else {
          session.cursor = update.cursor;
          rows.push({uid:update.uid,cursor:update.cursor});
        }
      }
      if (rows.length) syncSidebarUpdates(rows);
    }
  } finally { uiEventApplying = false; }
}
function closeUiEvents() {
  clearTimeout(uiEventsRetry);
  uiEvents?.close(); uiEvents = null; uiEventsReady = false;
}
function startUiEvents() {
  if (SessionDockNetwork.paused) return;
  if (!SessionDockCapabilities.config.ui_events || !window.EventSource || document.hidden || uiEvents) return;
  clearTimeout(uiEventsRetry);
  const stream = new EventSource(appUrl('api/events'));
  uiEvents = stream;
  stream.addEventListener('change', event => {
    if (uiEvents !== stream) return;
    try {
      const change = JSON.parse(event.data);
      uiEventsReady = !change.retry;
      if (!change.retry) queueUiChange(change);
    } catch {
      closeUiEvents();
      if (!document.hidden) uiEventsRetry = setTimeout(startUiEvents, 3000);
    }
  });
  stream.onerror = () => {
    if (uiEvents !== stream) return;
    closeUiEvents();
    // Polling is a disconnected/old-server fallback, never parallel upkeep
    // of a healthy push connection. Reconnect sends a fresh baseline.
    if (!document.hidden) uiEventsRetry = setTimeout(startUiEvents, 3000);
  };
}
document.addEventListener('visibilitychange', () => {
  if (document.hidden) closeUiEvents(); else startUiEvents();
});
addEventListener('pagehide', closeUiEvents);
addEventListener('pageshow', startUiEvents);
setTimeout(startUiEvents, 0);

addEventListener('sessiondock-network-paused', event => {
  closeUiEvents();
  closeWatch();
  sessionPollController?.abort();
  clearTimeout(sessionLoadRetry);
  if (event.detail !== 'login' || $('.login-expired')) return;
  const notice = el('div', 'app-float warn login-expired');
  notice.setAttribute('role', 'alert');
  notice.innerHTML = '<div class="app-float-head"><strong>登录已失效</strong></div>'
    + '<span>自动同步已暂停。请在新窗口登录，再重新加载本页；未保存的输入仍保留在当前页面。</span>';
  const login = el('button', 'btn primary', '打开登录页');
  login.type = 'button'; login.onclick = () => window.open(APP_BASE, '_blank', 'noopener');
  const actions = el('div', 'app-float-actions'); actions.append(login); notice.append(actions);
  floatStack().append(notice);
});

addEventListener('sessiondock-network-resumed', async () => {
  await checkServerBuild();
  if (SessionDockNetwork.paused) return;
  startUiEvents();
  void loadSessions();
  void pollLive(true);
  void flushBrowserAudit();
  if (S.sel) {
    const uid = S.sel, agent = S.agent;
    await syncSession(uid, agent);
    if (S.sel === uid && S.agent === agent) watchSession(uid, agent);
  }
});

function visible() {
  const eligible = s => (!sessionHidden(s) || s.uid === S.sel) && !S.off.has(s.source)
    && nodeSelected(s) && (globalThis.SessionDockGroups?.matches(s) ?? true);
  let pool = (S.results || sidebarSessions()).filter(eligible);
  if (S.activeOnly) pool = pool.filter(s => s.pending || S.live.has(s.uid));
  if (S.term && S.results === null) pool = pool.filter(s => sidebarMainMatches(s)
    || sidebarAgentItems(s).length > 0);
  if (S.term && S.nest && S.view !== 'group') {
    // Retain only the ancestors needed to place matched rows in the tree.
    // They carry structural metadata, never a child's snippet or siblings.
    const found = new Map(pool.map(s => [s.uid, s]));
    const parents = new Map();
    const {children} = nestEdges(sidebarSessions().filter(eligible));
    for (const [uid, rows] of children) for (const row of rows) parents.set(row.uid, uid);
    const indexed = indexedSessions().byUid;
    for (const match of pool) {
      const seen = new Set([match.uid]);
      let uid = parents.get(match.uid);
      while (uid && !seen.has(uid)) {
        seen.add(uid);
        if (!found.has(uid)) {
          const parent = indexed.get(uid);
          if (parent) found.set(uid, {...parent, hits: 0, hits_capped: false, snippet: '', agent_items: []});
        }
        uid = parents.get(uid);
      }
    }
    pool = [...found.values()];
  }
  return pool;
}

// ---------------------------------------------------------------- 左栏
const sessionStarred = uid => !!indexedSessions().byUid.get(uid)?.starred;

/* ---------- 左栏多选操作 ---------- */
// 已有记录移入回收站；未落盘的新建会话停止并丢弃，正式运行会话由服务端拒绝删除。
const pickedSessions = new Set();
const sessionPickable = session => !session.fork_parent;
let sessionDeleteBusy = false;
let sessionStopBusy = false;
let sessionStopProgress = null;
const STOP_STAGE_TEXT = SessionDockSessionUi.STOP_STAGE_TEXT;
function sessionStopConcurrency() {
  const value = Number(store.get('stopConcurrency', 6));
  return [1, 2, 4, 6, 8, 12, 16].includes(value) ? value : 6;
}

function pickedStopTargets() {
  return sidebarSessions().filter(s => pickedSessions.has(s.uid) && sessionPickable(s)
    && (s.pending ? sessionStopCapable() && !!s.record_id && !!s.instance_id
      && !s.stale && !['exited', 'failed'].includes(s.state) && s.running !== false
      : sessionStoppable(s.uid)));
}

/** 列表会被 SSE/轮询整份重画，选择集合里只保留仍然存在且可删的会话。 */
function syncPickedSessions() {
  if (!S.picking) {
    pickedSessions.clear();
    return pickedSessions;
  }
  const alive = new Set(sidebarSessions().filter(sessionPickable).map(s => s.uid));
  for (const uid of [...pickedSessions]) if (!alive.has(uid)) pickedSessions.delete(uid);
  return pickedSessions;
}

function setPicking(on) {
  if (sessionStopBusy) return;
  sessionStopProgress = null; S.picking = !!on;
  if (!S.picking) pickedSessions.clear();
  if (S.picking) {S.nestAttach = ''; S.nestAttachUids = [];}
  renderPickBar(); renderSide();
}

function toggleSessionPick(uid) {
  if (sessionStopBusy) return;
  sessionStopProgress = null;
  pickedSessions.has(uid) ? pickedSessions.delete(uid) : pickedSessions.add(uid);
  renderPickBar();
}

/** 整组一起勾/取消：组内还有没选中的就补齐，已经全选才清空。 */
function toggleGroupPick(uids, group) {
  if (sessionStopBusy) return;
  sessionStopProgress = null;
  const all = uids.length && uids.every(uid => pickedSessions.has(uid));
  for (const uid of uids) all ? pickedSessions.delete(uid) : pickedSessions.add(uid);
  renderPickBar();
}

/** 多选模式下按住鼠标左键划过一片：按下的那条原来没选中就整片选中，原来已选中
 *  就整片取消；划回去恢复按下前的状态，划到列表上下边缘自动滚动。只动了一条时
 *  仍走普通点击。 */
let pickDrag = null, pickDragSwallowClick = false;
function pickDragRows() {
  return [...$('#side').querySelectorAll('.item[data-uid]')]
    .filter(row => row.querySelector('.item-pick') && row.getClientRects().length);
}
function pickDragTo(row) {
  const uid = row?.dataset.uid;
  if (!pickDrag || !uid || uid === pickDrag.last) return;
  const rows = pickDragRows();
  const from = rows.findIndex(r => r.dataset.uid === pickDrag.anchor), to = rows.indexOf(row);
  if (from < 0 || to < 0) return;
  pickDrag.last = uid;
  pickDrag.moved = true;
  sessionStopProgress = null;
  pickedSessions.clear();
  for (const kept of pickDrag.before) pickedSessions.add(kept);
  for (let i = Math.min(from, to); i <= Math.max(from, to); i++) {
    pickDrag.on ? pickedSessions.add(rows[i].dataset.uid) : pickedSessions.delete(rows[i].dataset.uid);
  }
  renderPickBar();
}
/** 指针所在高度上可见的那一行；落在组标题、间隙或列表外时取纵向最近的可见行。 */
function pickDragAt(y) {
  const side = $('#side').getBoundingClientRect();
  let best = null, gap = Infinity;
  for (const row of pickDragRows()) {
    const box = row.getBoundingClientRect();
    if (box.bottom <= side.top || box.top >= side.bottom) continue;
    const distance = y < box.top ? box.top - y : y > box.bottom ? y - box.bottom : 0;
    if (distance < gap) { best = row; gap = distance; }
    if (!distance) break;
  }
  return best;
}
function pickDragScroll() {
  if (!pickDrag) return;
  const side = $('#side'), box = side.getBoundingClientRect(), edge = 36;
  const over = pickDrag.y < box.top + edge ? pickDrag.y - box.top - edge
    : pickDrag.y > box.bottom - edge ? pickDrag.y - box.bottom + edge : 0;
  if (over) {
    side.scrollTop += Math.max(-24, Math.min(24, Math.round(over / 2)));
    pickDragTo(pickDragAt(pickDrag.y));
  }
  pickDrag.frame = requestAnimationFrame(pickDragScroll);
}
function endPickDrag() {
  if (!pickDrag) return;
  cancelAnimationFrame(pickDrag.frame);
  if (pickDrag.moved) {
    // 松手处的 click 不能再把按下那条切回去。
    pickDragSwallowClick = true;
    setTimeout(() => { pickDragSwallowClick = false; }, 0);
  }
  pickDrag = null;
}
function sidebarGestureMousedown1(event) {
  if (event.button !== 0 || event.shiftKey || event.ctrlKey || event.metaKey || event.altKey) return;
  if (!S.picking || S.nestAttach || sessionStopBusy) return;
  const row = event.target.closest('.item[data-uid]');
  if (!row?.querySelector('.item-pick') || event.target.closest('.item-star, .nest-caret')) return;
  event.preventDefault();   // 划选不拉出文字选区
  endPickDrag();
  pickDrag = {anchor: row.dataset.uid, last: row.dataset.uid, on: !pickedSessions.has(row.dataset.uid),
              before: new Set(pickedSessions), moved: false, y: event.clientY};
  pickDrag.frame = requestAnimationFrame(pickDragScroll);
}
addEventListener('mousemove', event => {
  if (!pickDrag) return;
  if (!(event.buttons & 1)) { endPickDrag(); return; }
  pickDrag.y = event.clientY;
  pickDragTo(pickDragAt(event.clientY));
});
addEventListener('mouseup', event => { if (event.button === 0) endPickDrag(); });
addEventListener('blur', endPickDrag);
// 多选时列表里不拉文字选区（含触屏长按）。不用 #side.picking 的 CSS：切换它要为
// 上千行重算样式，进出多选会多卡一两百毫秒。
function sidebarGestureSelectstart1(event) {
  if (S.picking) event.preventDefault();
}
function sidebarGestureClick1(event) {
  if (!pickDragSwallowClick) return;
  pickDragSwallowClick = false;
  event.stopPropagation();
  event.preventDefault();
}

function pickAllVisible() {
  if (sessionStopBusy) return;
  sessionStopProgress = null;
  const rows = visible().filter(sessionPickable);
  const all = rows.length && rows.every(s => pickedSessions.has(s.uid));
  pickedSessions.clear();
  if (!all) rows.forEach(s => pickedSessions.add(s.uid));
  const side = $('#side'), top = side.scrollTop;
  renderSide();
  side.scrollTop = top;              // 全选不该把列表弹回顶部
  renderPickBar();
}

function renderPickBar() {
  ensureSidebarVue();
  const attaching = !!S.nestAttach, picked = S.picking ? pickedSessions.size : 0;
  const row = attaching ? sidebarSessions().find(s => s.uid === S.nestAttach) : null;
  const pending = pendingTmuxSessions().filter(s => pickedSessions.has(s.uid)).length;
  const action = pending ? (pending === picked ? '丢弃' : '删除 / 丢弃') : '删除';
  const attachable = S.picking ? pickedNestable().length : 0;
  const stoppable = S.picking ? pickedStopTargets().length : 0, progress = sessionStopProgress;
  const rows = S.picking ? visible().filter(sessionPickable) : [];
  SessionDockSidebar.updatePick({hidden: !S.picking && !attaching, attaching,
    label: attaching ? (S.nestAttachUids.length > 1 ? `点击要附属的会话（当前：${S.nestAttachUids.length} 个会话）` : row ? `点击要附属的会话（当前：${row.title || S.nestAttach}）` : '点击要附属的会话') : picked ? `已选 ${picked} 项` : '点会话行勾选',
    groupHidden: attaching || !globalThis.SessionDockGroups?.available,
    groupDisabled: !!globalThis.SessionDockGroups?.busy || ![...pickedSessions].some(uid => {const row = indexedSessions().byUid.get(uid); return row && !row.pending;}),
    allDisabled: !rows.length || sessionStopBusy, allLabel: rows.length && rows.every(s => pickedSessions.has(s.uid)) ? '全不选' : '全选',
    cancelDisabled: sessionStopBusy, deleteLabel: picked ? `${action} (${picked})` : action,
    deleteDisabled: !picked || sessionDeleteBusy || sessionStopBusy,
    attachHidden: attaching || !SessionDockCapabilities.allows('metadata'), attachLabel: attachable ? `附属到… (${attachable})` : '附属到…',
    attachDisabled: !attachable || sessionDeleteBusy || sessionStopBusy,
    stopLabel: progress ? `已停止 ${progress.stopped}/${progress.total}` : stoppable ? `停止 (${stoppable})` : '停止',
    stopDisabled: !stoppable || sessionDeleteBusy || sessionStopBusy, stopBusy: sessionStopBusy,
    stopTitle: progress ? `已返回 ${progress.settled}/${progress.total}，失败 ${progress.failed}，未确认 ${progress.uncertain}` : '停止选中的运行中会话',
    detailsHidden: attaching || !S.picking || !progress?.details.length,
    summary: progress ? [progress.failed ? `失败 ${progress.failed}` : '', progress.uncertain ? `未确认 ${progress.uncertain}` : '', progress.refreshError ? '状态刷新失败' : ''].filter(Boolean).join('，') : '',
    errors: progress ? progress.details.join('\n') : ''});
  refreshSidebarRows();
  SessionDockSidebar.updateGroups(SessionDockSidebar.currentGroups().map(group => {
    const picked = group.pickUids.filter(uid => pickedSessions.has(uid)).length;
    return {...group, picked: !!group.pickUids.length && picked === group.pickUids.length, indeterminate: picked > 0 && picked < group.pickUids.length};
  }));
}

async function deleteSessions(uids, button = null) {
  if (!uids.length || sessionDeleteBusy || sessionStopBusy) return null;
  const pending = pendingTmuxSessions().filter(s => uids.includes(s.uid));
  const pendingIds = new Set(pending.map(s => s.uid));
  const recorded = uids.filter(uid => !pendingIds.has(uid));
  const action = pending.length ? (recorded.length ? '删除 / 丢弃' : '丢弃') : '删除';
  const only = uids.length === 1
    ? (sidebarSessions().find(x => x.uid === uids[0])?.title || '') : '';
  const running = recorded.filter(uid => S.live.has(uid)).length;
  // OpenCode keeps sessions in its own database: they are deleted there,
  // with their child sessions, and never reach the recycle bin.
  const opencode = recorded.filter(uid => sidebarSessions().find(x => x.uid === uid)?.source === 'opencode');
  // Unpersisted launches discard immediately. Recorded sessions still confirm
  // because they move into the recycle bin.
  if (recorded.length && !await appConfirm((uids.length === 1
      ? `${action}会话「${only}」?\n\n` : `${action}选中的 ${uids.length} 个会话?\n\n`)
    + (pending.length ? `${pending.length} 个新建会话将停止并丢弃，未发送的草稿也会清除；若已生成会话记录，记录会保留。\n` : '')
    + (opencode.length < recorded.length ? trashLocationNote() : '')
    + (opencode.length ? (opencode.length === recorded.length ? '' : '\n')
      + (opencode.length === 1 && uids.length === 1 ? '' : `其中 ${opencode.length} 个 `) + opencodeDeleteNote() : '')
    + (running ? `\n其中 ${running} 个还在运行，会被跳过，需要先停止。` : '')))
    return null;
  sessionDeleteBusy = true;
  if (button) button.disabled = true;
  const watched = recorded.includes(S.sel) ? S.sel : null;
  if (watched) closeWatch();   // 文件即将移走，先停掉这条 SSE
  const d = { deleted: [], errors: [] };
  try {
    for (const info of pending) {
      try {
        await discardPendingSession(info);
        d.deleted.push({ uid: info.uid });
      } catch (e) {
        d.errors.push({ uid: info.uid, title: info.title, error: e.message });
      }
    }
    if (recorded.length) {
      try {
        const postDelete = async (uids, force = false) => {
          const r = await fetch(appUrl('api/sessions/delete'), {
            method: 'POST', headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(force ? { uids, force: true } : { uids }),
          });
          const result = await r.json();
          if (!r.ok || result.error) throw new Error(result.error || `HTTP ${r.status}`);
          return result;
        };
        const result = await postDelete(recorded);
        d.deleted.push(...(result.deleted || []));
        let errors = result.errors || [];
        // Rust 回收站：运行状态未知的会话先被跳过，用户确认后才带 force 重试。
        const unknown = trashCapable() ? (result.skipped || []).filter(x => x.needs_force) : [];
        if (unknown.length && await confirmForceDelete(unknown.length, unknown[0].run_state?.detail)) {
          const forced = await postDelete(unknown.map(x => x.uid), true);
          d.deleted.push(...(forced.deleted || []));
          const retried = new Set(unknown.map(x => x.uid));
          errors = errors.filter(x => !retried.has(x.uid)).concat(forced.errors || []);
        }
        d.errors.push(...errors);
      } catch (e) {
        d.errors.push(...recorded.map(uid => ({ uid, error: e.message })));
      }
    }
    if (pending.length) await loadTermList();
  } finally {
    sessionDeleteBusy = false;
    if (button) button.disabled = false;
  }
  const gone = new Set((d.deleted || []).map(x => x.uid));
  const failed = d.errors || [];
  if (watched && S.sel === watched && !gone.has(watched)) watchSession(watched, S.agent);
  if (gone.size) {
    forgetDeletedReceipts(S.sessions.filter(x => gone.has(x.uid)));
    S.sessions = S.sessions.filter(x => !gone.has(x.uid));
    if (S.results) S.results = S.results.filter(x => !gone.has(x.uid));
    if (gone.has(S.sel)) {
      S.sel = null;
      store.set('sel', null);
      const trashed = !opencode.includes(watched || '');
      $('#detail').innerHTML = trashed ? '<div class="empty">已移入回收站'
        + '<br><button type="button" class="btn" id="detail-open-trash">打开回收站</button></div>'
        : '<div class="empty">会话已从 OpenCode 删除</div>';
      if (trashed) $('#detail-open-trash').onclick = openTrash;
      ensureConsolePlaceholder();
      auditDetailRendered('trashed');
      showMobileList();
    }
  }
  renderChips();
  renderSide();
  if (failed.length === 1 && uids.length === 1) {
    await appAlert(`${action}失败: ` + failed[0].error);
  } else if (failed.length) {
    const lines = failed.slice(0, 5).map(x => `· ${x.title || x.uid}: ${x.error}`);
    await appAlert(`已${action} ${gone.size} 个，${failed.length} 个操作失败:\n\n`
      + lines.join('\n') + (failed.length > 5 ? '\n…' : ''));
  }
  return { gone, failed };
}

async function deletePickedSessions() {
  const result = await deleteSessions([...pickedSessions], $('#side-pick-delete'));
  if (!result) return;
  // 删不掉的（多半还在运行）留在选择里，用户停掉会话后可以直接再点删除。
  pickedSessions.clear();
  result.failed.forEach(x => pickedSessions.add(x.uid));
  if (result.failed.length) { renderSide(); renderPickBar(); } else setPicking(false);
}

async function stopPickedSessions() {
  if (sessionStopBusy || sessionDeleteBusy) return;
  const targets = pickedStopTargets();
  if (!targets.length) return;
  // 全部停在输入框（空闲）时直接停；有在轮转、等回答或状态未知的才确认。
  if (!targets.every(s => !s.pending && sessionTurn(s.uid) === 'idle')
      && !await appConfirm(`停止所选的 ${targets.length} 个运行中会话?\n\n会话记录和草稿会保留，已结束的会话会跳过。`)) return;
  sessionStopBusy = true;
  const progress = sessionStopProgress = {total: targets.length, stopped: 0, settled: 0,
    failed: 0, uncertain: 0, details: [], refreshError: false};
  showSessionStopNotice('');
  $('#side-stop-details').open = false;
  renderPickBar();
  let next = 0;
  const stopNext = async () => {
    while (next < targets.length) {
      const target = targets[next++];
      try {
        if (target.pending) {
          const result = await post('api/term/kill', { record_id: target.record_id,
            instance_id: target.instance_id, ...(HUB_MODE ? { _node: target.node_id } : {}) });
          if (result.error) throw new Error(result.error);
          const current = T.pending.find(row => row.record_id === target.record_id && row.node_id === target.node_id);
          if (current) Object.assign(current, result);
          if (!['exited', 'failed'].includes(result.state)) {
            progress.uncertain++;
            progress.details.push(`「${target.title}」停止请求已发送，尚未确认退出`);
            continue;
          }
        } else {
          const result = await requestSessionStop(target);
          if (result.stage === 'uncertain') {
            progress.uncertain++;
            progress.details.push(`「${target.title}」${STOP_STAGE_TEXT.uncertain}`);
            continue;
          }
        }
        progress.stopped++;
      } catch (error) {
        progress.failed++;
        progress.details.push(`「${target.title}」停止失败：${error.message || error}`);
      } finally {
        progress.settled++;
        renderPickBar();
      }
    }
  };
  try {
    await Promise.all(Array.from({length: Math.min(sessionStopConcurrency(), targets.length)}, stopNext));
    await refreshLive(true);
    if (typeof loadTermList === 'function') await loadTermList();
    paintLive();
  } catch (error) {
    progress.refreshError = true;
    progress.details.push(`停止请求已处理，刷新状态失败：${error.message || error}`);
  } finally {
    sessionStopBusy = false;
    renderPickBar();
  }
}







/** 选中的会话里能改附属关系的：真实会话行，不是待定启动或分叉父行。 */
function pickedNestable() {
  if (!SessionDockCapabilities.allows('metadata')) return [];
  const rows = new Map(sidebarSessions().map(session => [session.uid, session]));
  return [...pickedSessions].filter(uid => {
    const row = rows.get(uid);
    return !!row && !row.pending && !row.fork_parent;
  });
}

/* ---------- 会话行的右键 / 长按菜单 ---------- */
// 删除入口不再常驻占位：右键（手机长按）某条会话，才给出删除和进入多选。
const LONG_PRESS_MS = 480;
const LONG_PRESS_SLOP = 12;   // 手指按住时的自然微动不该算滑动
let menuUid = '';
let longPress = { timer: 0, x: 0, y: 0 };
let suppressItemClick = false;

function openItemMenu(uid, x, y) {
  const menu = $('#item-menu');
  globalThis.SessionDockGroups?.closeMenu();
  menuUid = uid;
  const list = sidebarSessions();
  const row = list.find(session => session.uid === uid);
  const parent = !!row?.fork_parent;
  const running = sessionStoppable(uid);
  const unusedLaunch = !row?.pending && unusedNewAssignedLaunch(row);
  // 运行中的 SSH 会话先停止，结束后才删除；未使用的原生启动仍可直接丢弃。
  const shellRunning = typeof pendingShellRunning === 'function' && pendingShellRunning(row);
  const nested = !!(row?.nest_parent?.source && row.nest_parent.sid);
  const nestable = SessionDockCapabilities.allows('metadata') && !!row && !row.pending && !parent;
  // Keep every action in its fixed position, with the same unavailable hints as
  // other controls. The shared capture handler blocks mouse, touch and keyboard clicks.
  const unavailable = {
    'copy-identity': !row?.sid ? '此会话尚无原生会话标识。' : '',
    stop: parent || (row?.pending ? !shellRunning : !running || !!unusedLaunch)
      ? '此会话当前没有可停止的进程。' : '',
    hide: !parent ? '仅分叉父会话可隐藏。' : '',
    detach: !nestable || !nested ? '此会话当前没有附属关系。' : '',
    attach: !nestable ? '此会话当前不能设置附属关系。' : '',
    delete: parent ? '分叉父会话可隐藏，不能直接删除。'
      : ((!row?.pending && running && !unusedLaunch) || shellRunning)
        ? '请先停止会话再删除。' : '',
    group: !SessionDockCapabilities.allows('metadata') || !row || row.pending || !globalThis.SessionDockGroups?.available
      ? '此会话当前不能保存分组。' : '',
    pick: parent ? '分叉父会话不能加入多选。' : '',
  };
  for (const button of menu.querySelectorAll('button[data-act]')) {
    button.hidden = false;
    if (button.dataset.act !== 'clone') setControlUnavailable(button, unavailable[button.dataset.act]);
  }
  menu.querySelector('[data-act="delete"]').textContent = ((row?.pending && row?.source !== 'shell') || unusedLaunch) ? '丢弃会话' : '删除会话';
  paintTransferAvailability(menu.querySelector('[data-act="clone"]'), uid);
  menu.hidden = false;
  const box = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(8, Math.min(x, innerWidth - box.width - 8))}px`;
  menu.style.top = `${Math.max(8, Math.min(y, innerHeight - box.height - 8))}px`;
  menu.querySelector('button:not([aria-disabled="true"])')?.focus({ preventScroll: true });
}

function closeItemMenu() {
  globalThis.SessionDockGroups?.closeMenu();
  $('#item-menu').hidden = true;
  menuUid = '';
}

function cancelLongPress() {
  clearTimeout(longPress.timer);
  longPress.timer = 0;
}

const menuTarget = e => e.target.closest('#side .item');

// 桌面端允许从会话标题、目录和元信息里复制文字。拖选结束时浏览器仍会派发
// click；这不是打开会话的意图，而且重画左栏会立即销毁刚建立的 Selection。
// 自动列表更新也要等用户清掉选区后再画，否则活跃会话一刷新，蓝色选区就消失。
let sidebarRenderDeferred = false;
let sidebarRenderFlushTimer = 0;
let sidebarTextPointer = null;

function sidebarTextSelectionActive() {
  const selection = getSelection();
  const side = $('#side');
  return !!selection && !selection.isCollapsed && !!selection.toString()
    && !!side && side.contains(selection.anchorNode) && side.contains(selection.focusNode);
}

const sidebarTextSelectionProtected = () => !!sidebarTextPointer || sidebarTextSelectionActive();

function scheduleDeferredSidebarRender() {
  if (!sidebarRenderDeferred || sidebarTextSelectionProtected() || sidebarRenderFlushTimer) return;
  sidebarRenderFlushTimer = setTimeout(flushDeferredSidebarRender, 0);
}

function flushDeferredSidebarRender() {
  sidebarRenderFlushTimer = 0;
  if (!sidebarRenderDeferred || sidebarTextSelectionProtected()) return;
  const side = $('#side');
  const top = side?.scrollTop || 0;
  sidebarRenderDeferred = false;
  renderSide();
  if (side) side.scrollTop = top;
  paintLive();
}

document.addEventListener('selectionchange', () => {
  scheduleDeferredSidebarRender();
});

function sidebarGesturePointerdown1(event) {
  if (event.pointerType !== 'mouse' || event.button !== 0 || S.picking
      || !event.target.closest('.t, .m, .cwd, .snip, .gname')) return;
  sidebarTextPointer = {id: event.pointerId, x: event.clientX, y: event.clientY, moved: false};
}

document.addEventListener('pointermove', event => {
  if (longPress.timer && $('#side').contains(event.target)) sidebarGesturePointermove1(event);
  if (!sidebarTextPointer || sidebarTextPointer.id !== event.pointerId) return;
  if (Math.abs(event.clientX - sidebarTextPointer.x) > 3
      || Math.abs(event.clientY - sidebarTextPointer.y) > 3) sidebarTextPointer.moved = true;
});

document.addEventListener('pointerup', event => {
  if (longPress.timer && $('#side').contains(event.target)) cancelLongPress();
  if (!sidebarTextPointer || sidebarTextPointer.id !== event.pointerId) return;
  const pointer = sidebarTextPointer;
  setTimeout(() => {
    if (sidebarTextPointer !== pointer) return;
    sidebarTextPointer = null;
    scheduleDeferredSidebarRender();
  }, 0);
});

document.addEventListener('pointercancel', event => {
  if (longPress.timer && $('#side').contains(event.target)) cancelLongPress();
  if (!sidebarTextPointer || sidebarTextPointer.id !== event.pointerId) return;
  sidebarTextPointer = null;
  scheduleDeferredSidebarRender();
});

function sidebarGestureClick2(event) {
  const dragged = !!sidebarTextPointer?.moved;
  sidebarTextPointer = null;
  if (!dragged && !sidebarTextSelectionActive()) {
    scheduleDeferredSidebarRender();
    return;
  }
  event.stopPropagation();
  event.preventDefault();
}

function sidebarGestureContextmenu1(e) {
  const row = menuTarget(e);
  if (!row || S.picking || S.nestAttach) return;      // 选择/附属点选里点选就够了，不再叠一层菜单
  e.preventDefault();
  openItemMenu(row.dataset.uid, e.clientX, e.clientY);
}

function sidebarGesturePointerdown2(e) {
  if (e.pointerType === 'mouse') return;             // 鼠标走 contextmenu
  const row = menuTarget(e);
  if (!row || S.picking || S.nestAttach) return;
  cancelLongPress(); // A second finger must not leave the first hold timer alive.
  longPress = { timer: 0, x: e.clientX, y: e.clientY };
  longPress.timer = setTimeout(() => {
    longPress.timer = 0;
    suppressItemClick = true;                        // 长按不该顺手打开会话
    navigator.vibrate?.(12);
    openItemMenu(row.dataset.uid, longPress.x, longPress.y);
  }, LONG_PRESS_MS);
}

function sidebarGesturePointermove1(e) {
  if (!longPress.timer) return;
  if (Math.abs(e.clientX - longPress.x) > LONG_PRESS_SLOP
      || Math.abs(e.clientY - longPress.y) > LONG_PRESS_SLOP) cancelLongPress();
}

$('#side').addEventListener('scroll', () => {cancelLongPress(); closeItemMenu();});

// 长按结束时浏览器仍会补一次 click，必须在捕获阶段吃掉。
function sidebarGestureClick3(e) {
  if (!suppressItemClick) return;
  suppressItemClick = false;
  e.stopPropagation();
  e.preventDefault();
}

$('#item-menu').onclick = async e => {
  const button = e.target.closest('button[data-act]');
  if (!button || button.getAttribute('aria-disabled') === 'true') return;
  const uid = menuUid;
  if (button.dataset.act === 'group') { globalThis.SessionDockGroups?.showMenu([uid], button); return; }
  closeItemMenu();
  if (!uid) return;
  if (button.dataset.act === 'copy-identity') {
    const row = sidebarSessions().find(session => session.uid === uid);
    if (!row?.sid) return;
    const machine = row.node_name || Nodes.list.find(node => node.id === row.node_id)?.name
      || (!HUB_MODE ? serverHostname : '') || '(未知)';
    const text = `机器：${machine}\n目录：${row.cwd || '(未知)'}\nagent：${row.source}\nUUID：${row.sid}`;
    try {
      await copyFileText(text);
      showSessionStopNotice('会话标识已复制。');
    } catch (error) {
      await appAlert(`复制会话标识失败：${error.message || error}`);
    }
    return;
  }
  if (button.dataset.act === 'clone') { await cloneSessionGroup(uid); return; }
  if (button.dataset.act === 'hide') {
    await setForkParentVisibility([uid], false);
    return;
  }
  if (button.dataset.act === 'detach') {
    await setSessionNest(uid, {parent_uid: null});
    return;
  }
  if (button.dataset.act === 'attach') {
    setNestAttach(uid);
    return;
  }
  if (button.dataset.act === 'pick') {
    pickedSessions.add(uid);       // 从哪条进入多选，就先勾上哪条
    setPicking(true);
    return;
  }
  if (button.dataset.act === 'stop') {
    const row = S.sessions.find(x => x.uid === uid) || sidebarSessions().find(x => x.uid === uid);
    if (row?.pending) { if (typeof stopPendingSession === 'function') await stopPendingSession(row); return; }
    if (row) await stopSession(row);
    return;
  }
  const row = sidebarSessions().find(session => session.uid === uid);
  const launch = unusedNewAssignedLaunch(row);
  if (launch && typeof deletePendingSession === 'function') {
    await deletePendingSession(launch);
    return;
  }
  await deleteSessions([uid]);
};

document.addEventListener('pointerdown', e => {
  if (!$('#item-menu').hidden && !e.target.closest('#item-menu, #session-group-menu')) closeItemMenu();
}, true);
addEventListener('resize', closeItemMenu);



function paintStarButton(...args) {return SessionUiApp.paintStarButton(...args);}

function applySessionStar(uid, starred, starredAt = null) {
  for (const rows of [S.sessions, S.results || []]) {
    const row = rows.find(s => s.uid === uid);
    if (!row) continue;
    row.starred = starred;
    if (starredAt) row.starred_at = starredAt;
    else delete row.starred_at;
  }
  for (const entry of cache.values()) {
    if (entry.meta?.uid !== uid) continue;
    entry.meta.starred = starred;
    if (starredAt) entry.meta.starred_at = starredAt;
    else delete entry.meta.starred_at;
  }
}

function applyForkParentVisibility(uid, visible) {
  for (const rows of [S.sessions, S.results || []]) {
    const row = rows.find(session => session.uid === uid);
    if (row?.fork_parent) row.fork_parent_visible = visible;
  }
  for (const entry of cache.values()) {
    if (entry.meta?.uid === uid && entry.meta.fork_parent) {
      entry.meta.fork_parent_visible = visible;
    }
  }
}

async function setForkParentVisibility(uids, visible, button = null) {
  const unique = [...new Set(uids)].filter(Boolean);
  if (!unique.length) return {updated: [], errors: []};
  if (button) button.disabled = true;
  try {
    const response = await fetch(appUrl('api/sessions/fork-visibility'), {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({uids: unique, visible}),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || `HTTP ${response.status}`);
    for (const row of data.updated || []) {
      applyForkParentVisibility(row.uid, !!row.fork_parent_visible);
    }
    renderNodes();
    renderChips();
    renderSide();
    showSessionCount();
    const selected = S.sessions.find(session => session.uid === S.sel);
    if (selected) renderSessionAction(selected);
    renderForkChainMenu();
    if (data.errors?.length) {
      await appAlert(`父会话显示状态有 ${data.errors.length} 项未保存：${data.errors[0].error}`);
    }
    return data;
  } catch (error) {
    await appAlert('父会话显示状态保存失败: ' + error.message);
    return null;
  } finally {
    if (button) button.disabled = false;
  }
}

function refreshStarPresentation(uid) {
  const side = $('#side');
  const top = side?.scrollTop || 0;
  renderSide();
  if (side) side.scrollTop = top;
  if (S.sel === uid) paintStarButton($('#a-star'), sessionStarred(uid), S.starBusy.has(uid));
}

async function toggleSessionStar(uid) {
  if (!uid || S.starBusy.has(uid)) return;
  const before = sessionStarred(uid);
  const wanted = !before;
  S.starBusy.add(uid);
  applySessionStar(uid, wanted);
  refreshStarPresentation(uid);
  try {
    const response = await fetch(appUrl('api/session/star'), {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({uid, starred: wanted}),
    });
    const data = await response.json();
    if (!response.ok) throw new Error(data.error || `HTTP ${response.status}`);
    applySessionStar(uid, !!data.starred, data.starred_at || null);
  } catch (error) {
    applySessionStar(uid, before);
    const stat = SessionDockSearch.status;
    if (stat) {
      stat.textContent = ' 星标保存失败';
      stat.classList.add('err');
      setTimeout(() => { stat.classList.remove('err'); showSessionCount(sidebarSessions().length); }, 1800);
    }
    console.error('星标保存失败', error);
  } finally {
    S.starBusy.delete(uid);
    refreshStarPresentation(uid);
  }
}

/* ---------- Claude 时间线固定显示（Rust 只读迁移能力） ----------
 * 只改 SessionDock的显示时间线；不写原生记录，也不给 CLI 发任何回滚信号。
 * 没有这个能力声明的页面，保持原有双 Esc 原生回滚流程。 */
function timelinePinEnabled() {
  return SessionDockCapabilities.config.backend === 'rust'
    && SessionDockCapabilities.config.timeline_pin === true;
}

function timelinePinFailed(message) {
  const stat = SessionDockSearch.status;
  if (!stat) return;
  stat.textContent = ` ${message}`;
  stat.classList.add('err');
  setTimeout(() => { stat.classList.remove('err'); showSessionCount(sidebarSessions().length); }, 2400);
}

const timelinePinBusy = new Set();
async function pinTimeline(uid, target) {
  if (!timelinePinEnabled() || !uid || timelinePinBusy.has(uid)) return false;
  timelinePinBusy.add(uid);
  try {
    const response = await fetch(appUrl('api/session/rewind'), {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({uid, target: target ?? null}),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || `HTTP ${response.status}`);
    // 固定/取消固定改变逻辑时间线：即便 JSONL 一个字节没变，服务端也会给
    // reset；用现有增量接口原子替换缓存和 DOM。
    S.lastSync = 0;
    await syncSession(uid, null);
    return true;
  } catch (error) {
    timelinePinFailed(`${target ? '固定显示失败' : '取消固定失败'}: ${error.message}`);
    console.error('时间线固定失败', error);
    return false;
  } finally {
    timelinePinBusy.delete(uid);
  }
}

function renderTimelinePinNotice(meta) {
  $('#timeline-pin-notice')?.remove();
  if (!timelinePinEnabled() || !meta || meta.agent_id || !meta.timeline_pin
      || typeof meta.timeline_pin !== 'object') return;
  const detail = $('#detail');
  if (!detail) return;
  const pin = meta.timeline_pin;
  // A rewind made in the terminal: the next native input settles it, so a
  // retired one needs no notice and an active one offers nothing to undo.
  if (pin.cli && pin.retired) return;
  const notice = document.createElement('div');
  notice.id = 'timeline-pin-notice';
  notice.setAttribute('role', 'status');
  notice.dataset.retired = String(!!pin.retired);
  if (pin.retired_reason) notice.dataset.retiredReason = String(pin.retired_reason);
  notice.style.cssText = 'display:flex;flex-wrap:wrap;align-items:center;gap:6px 10px;'
    + 'padding:8px 12px;flex:none;border-bottom:1px solid var(--border);font-size:13px';
  const text = document.createElement('span');
  const heading = detail.querySelector(':scope > .dhead');
  const place = () => { if (heading) heading.after(notice); else detail.prepend(notice); };
  if (pin.cli) {
    notice.dataset.cli = 'true';
    text.textContent = '已同步终端里的回滚，显示到回滚点为止';
    notice.append(text);
    place();
    return;
  }
  text.textContent = pin.retired
    ? `固定显示已失效：${pin.retired_message || pin.retired_reason || '原生记录已变化'}。CLI 未回滚。`
    : '已固定显示到所选输入之前，CLI 未回滚；原生记录继续后自动失效。';
  const clear = document.createElement('button');
  clear.type = 'button'; clear.className = 'btn';
  clear.id = 'timeline-pin-clear';
  clear.textContent = pin.retired ? '清除记录' : '取消固定';
  clear.title = '只移除 SessionDock 的显示固定，不会回滚 CLI';
  clear.onclick = () => { clear.disabled = true; void pinTimeline(meta.uid, null).finally(() => { clear.disabled = false; }); };
  notice.append(text, clear);
  place();
}



function applySourceFilterChange() {
  store.set('off', [...S.off]);
  renderChips(); renderSide();
  if (HUB_MODE && S.results !== null) void runSearch();
}

function selectOnlySource(source) {
  if (!Object.hasOwn(SOURCES, source)) return false;
  const control = document.querySelector(`#chips button[data-source="${CSS.escape(source)}"]`);
  if (control?.dataset.unavailableReason) { return false; }
  S.off = new Set(Object.keys(SOURCES).filter(item => item !== source));
  applySourceFilterChange();
  return true;
}

function selectOnlyNodeFilter(id) {
  const node = Nodes.list.find(item => item.id === id);
  if (!node) return false;
  if (node.online === false) { return false; }
  Nodes.off = new Set(Nodes.list.filter(item => item.id !== id).map(item => item.id));
  store.set('nodesOff', [...Nodes.off]);
  renderNodes(); renderChips(); renderSide();
  showSessionCount(sidebarSessions().filter(nodeSelected).length);
  if (S.results !== null) void runSearch();
  return true;
}

function renderChips() {
  ensureSidebarVue(); const counts = new Map();
  for (const row of sidebarSessions()) if (nodeSelected(row)) counts.set(row.source, (counts.get(row.source) || 0) + 1);
  SessionDockSidebar.updateSources(Object.entries(SOURCES).map(([key, value]) => ({
    key, label: value.name, count: counts.get(key) || 0, on: !S.off.has(key), icon: value.icon, color: value.color,
    title: `${value.name}：点击选择或取消；右键或长按只选此类型`,
    reason: !counts.get(key) ? `${value.name} 在当前选择的机器上没有会话。` : ''})));
}

/* 机器与 Agent Type 默认是多选；右键（桌面）或长按（触屏）快速收窄到一项。 */
function selectOnlyFilter(button) {
  if (button.dataset.unavailableReason) return false;
  if (button.dataset.node) return selectOnlyNodeFilter(button.dataset.node);
  if (button.dataset.source) return selectOnlySource(button.dataset.source);
  return false;
}

/* ---------- 分层：附属关系 ---------- */
const spawnKey = (nodeId, source, sid) => JSON.stringify([nodeId || '', source, String(sid)]);

/** SessionDock stores only the user-selected sidebar parent. */
function nestSpecParent(session) {
  return session.nest_parent?.source && session.nest_parent.sid
    ? {parent: session.nest_parent} : null;
}

function nestParentOf(session, byKey, allByKey = new Map(S.sessions.map(s => [spawnKey(s.node_id, s.source, s.sid), s]))) {
  return SessionDockSidebar.Grouping.parentOf(session, byKey, allByKey, sidebarGroupingContext());
}
function nestEdges(list) {return SessionDockSidebar.Grouping.nestEdges(list, sidebarGroupingContext());}
function nestTree(list) {return S.nest ? nestEdges(list) : {children: new Map(), nested: new Set()};}

function nestDescendantUids(uid, list = sidebarSessions()) {
  const {children} = nestEdges(list);
  const out = new Set();
  const walk = id => {
    for (const child of children.get(id) || []) {
      if (out.has(child.uid)) continue;
      out.add(child.uid);
      walk(child.uid);
    }
  };
  walk(uid);
  return out;
}

function applySessionNest(uid, nestParent) {
  for (const rows of [S.sessions, S.results || []]) {
    const row = rows.find(session => session.uid === uid);
    if (!row) continue;
    if (nestParent) row.nest_parent = nestParent;
    else delete row.nest_parent;
  }
}

function setNestAttach(value) {
  S.nestAttachUids = (Array.isArray(value) ? value : [value]).filter(Boolean);
  S.nestAttach = S.nestAttachUids[0] || '';
  if (S.nestAttach && S.picking) {
    S.picking = false;
    pickedSessions.clear();
  }
  const side = $('#side'), top = side.scrollTop;
  renderPickBar();
  renderSide();
  side.scrollTop = top;
}

async function setSessionNest(uid, {parent_uid = null} = {}) {
  try {
    const response = await fetch(appUrl('api/session/nest'), {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({uid, parent_uid}),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || `HTTP ${response.status}`);
    applySessionNest(uid, data.nest_parent || null);
    if (data.nest_parent && !S.nest) {
      S.nest = true;
      store.set('nest', true);
      renderView();
    }
    const side = $('#side'), top = side?.scrollTop || 0;
    renderSide();
    if (side) side.scrollTop = top;
    return data;
  } catch (error) {
    await appAlert('会话附属关系保存失败: ' + error.message);
    return null;
  }
}

async function pickNestParent(target) {
  const listed = new Set(sidebarSessions().map(session => session.uid));
  const uids = S.nestAttachUids.filter(uid => listed.has(uid));
  if (!uids.length) { setNestAttach(''); return; }
  if (!target?.uid || uids.includes(target.uid)) return;
  if (uids.some(uid => nestDescendantUids(uid).has(target.uid))) {
    await appAlert('不能附属到自己的子会话下面');
    return;
  }
  // 逐条保存；失败的留在点选里（setSessionNest 已提示原因），可以再点一次。
  const failed = [];
  for (const uid of uids) {
    if (!await setSessionNest(uid, {parent_uid: target.uid})) failed.push(uid);
  }
  setNestAttach(failed);
}

/** 一条会话连同它的子代理和它发起的会话，按活动时间倒序、还在跑的在前。 */
function nestStamp(s, children, memo = new Map()) {return SessionDockSidebar.Grouping.nestStamp(s, children, memo);}

// Each new search starts open; its folds never read or write saved list folds.
const sidebarGroupFolds = () => S.term ? S.searchClosed : S.closed;
const sidebarNestFolds = () => S.term ? S.searchNestClosed : S.nestClosed;
const sidebarGroupClosed = key => sidebarGroupFolds().has(key);
const sidebarNestClosed = uid => sidebarNestFolds().has(uid);

function sidebarSearchText(s, agent = null) {
  const row = agent || s;
  return [row.title, row.cwd || s.cwd, s.node_name, s.source, SOURCES[s.source]?.name,
    row.model, agent ? agent.type : s.agent_type,
    ...(agent ? [agent.id] : [s.sid, s.uid])].join('\n');
}

function sidebarMainMatches(s) {
  if (!S.term) return true;
  return S.results !== null ? s.hits > 0
    : matchesSearch(sidebarSearchText(s));
}

function sidebarAgentItems(s) {
  const agents = s.agent_items || [];
  if (!S.term) return agents;
  if (S.results !== null) return agents.filter(a => a.hits > 0);
  return agents.filter(a => matchesSearch(sidebarSearchText(s, a)));
}

function sidebarMatchCount(list) {
  return list.reduce((count, s) => count + Number(sidebarMainMatches(s)) + sidebarAgentItems(s).length, 0);
}

function sidebarRowSnippet(s, agent = null) {
  return agent ? agent.snippet : sidebarMainMatches(s) ? s.snippet : '';
}

// Count the rows a branch would expose without sorting or allocating row objects.
// A nested closed session contributes just its own row, matching expandRows.
function nestSize(s, children, memo = new Map()) {return SessionDockSidebar.Grouping.nestSize(s, children, sidebarGroupingContext(), memo);}
function expandRows(s, depth, children, out, seen, memo = new Map(), sizes = new Map()) {
  return SessionDockSidebar.Grouping.expandRows(s, depth, children, out, seen, sidebarGroupingContext(), memo, sizes);
}
const rowKey = row => SessionDockSidebar.Grouping.rowKey(row);
function groupBy(list, {skipClosed = false} = {}) {return SessionDockSidebar.Grouping.groupBy(list, sidebarGroupingContext(), skipClosed);}

/** 列表项的元信息行。子代理数不在这里显示：它们就是挂在会话下面的缩进行，收起时计入三角数字。 */
// Rust pending rows carry the receipt state; a finished instance says so
// instead of "waiting" (its `stale` is the receipt flag, not a hub cache).
const pendingMeta = s => `${fmtTime(s.updated)} · ${typeof pendingStateLabel === 'function'
  ? pendingStateLabel(s) : '等待首条消息'}`;
const rustPendingRow = s => !!s.pending && !!s.record_id && SessionDockCapabilities.config.backend === 'rust';
// Explicitly shown ancestors can share the leaf's title. Keep their native
// relationship visible instead of making a fork chain look like duplicate rows.
const forkMeta = s => {
  if (s.agent_id) return '';
  const branch = s.forked_from_id
    ? (Number.isInteger(s.fork_depth) && s.fork_depth > 0 ? `分叉 ${s.fork_depth}` : '分叉会话')
    : '';
  return s.fork_parent ? `父会话（${branch || '原始'}）` : branch;
};
const itemMeta = s => (s.stale && !rustPendingRow(s) ? '离线缓存 · ' : '') + (s.pending ? pendingMeta(s)
  : [forkMeta(s), fmtTime(s.updated), fmtSize(s.size), s.model || '',
                       s.hits ? `命中 ${s.hits}${s.hits_capped ? '+' : ''}` : '']
                      .filter(Boolean).join(' · '));

/** Share keyed reconciliation for explicit renders and background list updates. */
function patchSide(list) {
  renderSide(list);
  return true;
}

/* ---------- 子代理行 ---------- */
const agentMeta = (uid, a) => `子代理 · ${a.type} · ${fmtSpan(a.created, agentRunning(uid, a) ? null : a.updated)}`
  + (S.term && a.hits > 0 ? ` · 命中 ${a.hits}${a.hits_capped ? '+' : ''}` : '');

/** 每行前面的引导区：每一级祖先一根竖线，再一个放三角的槽位（叶子留空）。子代理行让
 *  平铺模式也有引导区；有子代理或发起的孩子就有三角，与分层开关无关。
 *  槽位与分组标题的三角同列，深一层的槽位正好落在上一层图标的下方。 */
function paintAgentStatus(node) {if (node) refreshSidebarRows(node.dataset.owner);}

/** Move `.sel` without rebuilding `#side`. Clicking a row used to call
 *  `renderSide()`, which wipes the list and lets the scrollbar jump even when
 *  membership is unchanged — worst at the bottom of a long date/nest list. */
function paintSidebarSelection(uid, agent = null) {
  const side = $('#side');
  const row = agent ? side.querySelector(`.item.agent[data-owner="${CSS.escape(uid)}"][data-agent="${CSS.escape(agent)}"]`) : side.querySelector(`.item[data-uid="${CSS.escape(uid)}"]`);
  if (!row) {const top = side.scrollTop; renderSide(); side.scrollTop = top; return false;}
  refreshSidebarRows(); return true;
}

function exitSidebarSearch() { return SessionDockSearch.exitSearch(); }
function paintSearchMode(list) { return SessionDockSearch.paintSearchMode(list); }

/** Update a row without discarding selection, focus or its existing elements. */
var sidebarVueMounted;
function ensureSidebarVue() {
  if (sidebarVueMounted) return;
  sidebarVueMounted = true;
  SessionDockSidebar.mount({
    rowClick: (r, event) => {
      if (sidebarTextSelectionActive()) { event.preventDefault(); return; }
      if (r.agent) { if (!S.nestAttach && !S.picking) openSession(r.s.uid, r.agent.id); return; }
      if (S.nestAttach) { void pickNestParent(r.s); return; }
      if (S.picking) { if (sessionPickable(r.s)) toggleSessionPick(r.s.uid); return; }
      r.s.pending ? openPendingSession(r.s) : openSession(r.s.uid);
    },
    star: toggleSessionStar, nestFold: toggleNestFold,
    groupFold: (key, event) => {
      if (sidebarTextSelectionActive()) { event.preventDefault(); return; }
      const folds = sidebarGroupFolds(); folds.has(key) ? folds.delete(key) : folds.add(key);
      if (!S.term) store.set('closed', [...S.closed]); renderSide();
    },
    groupPick: toggleGroupPick, createGroup: input => globalThis.SessionDockGroups.create(input),
    removeGroup: name => globalThis.SessionDockGroups.remove(name),
    editGroup: on => globalThis.SessionDockGroups.edit(on), exitSearch: exitSidebarSearch,
    pickAll: pickAllVisible, pickStop: stopPickedSessions,
    pickGroup: event => globalThis.SessionDockGroups.showMenu([...pickedSessions], event.currentTarget),
    pickAttach: () => setNestAttach(pickedNestable()), pickDelete: deletePickedSessions,
    pickCancel: () => S.nestAttach ? setNestAttach('') : setPicking(false),
    sourceClick: key => { S.off.has(key) ? S.off.delete(key) : S.off.add(key); applySourceFilterChange(); },
    nodeClick: id => { const n = Nodes.list.find(n => n.id === id); if (!n || n.online === false) return;
      Nodes.off.has(id) ? Nodes.off.delete(id) : Nodes.off.add(id); store.set('nodesOff', [...Nodes.off]);
      renderNodes(); renderChips(); renderSide(); showSessionCount(); if (S.results !== null) void runSearch(); },
    resourceOpen: session => globalThis.SessionDockResources?.open(session),
    resourceCells: (session, agent) => globalThis.SessionDockSidebarResources?.cells(session, agent) || [],
    rowMounted: SessionDockSidebar.observeRow, rowUnmounted: SessionDockSidebar.unobserveRow,
  }, sidebarVueGestures(), {selectOnly: selectOnlyFilter, holdMs: LONG_PRESS_MS, slop: LONG_PRESS_SLOP});
}
function sidebarGroupingContext() {
  return {view: S.view, nest: S.nest, term: S.term, picking: S.picking, live: S.live, sessions: S.sessions,
    groupClosed: sidebarGroupClosed, nestClosed: sidebarNestClosed, mainMatches: sidebarMainMatches,
    agents: sidebarAgentItems, agentRunning, pickable: sessionPickable, dayKey,
    names: globalThis.SessionDockGroups?.names || [], available: !!globalThis.SessionDockGroups?.available,
    contains: name => globalThis.SessionDockGroups.contains(name), continued: sessionContinued,
    byUid: uid => indexedSessions().byUid.get(uid), expand: (...args) => expandRows(...args), hiddenForkParent, forkChildren};
}
function sidebarStatus(s, agent = null) {
  if (agent) {
    const running = sidebarAgentRunning(s.uid, agent);
    return SessionDockSidebar.Presentation.statusView({agent: true, running});
  }
  const unread = unreadRow(s.uid);
  const pending = sidebarPendingRunning(s);
  const draft = typeof composerDrafts !== 'undefined' ? composerDrafts.get(composerDraftOwner(s.uid)) : null;
  const active = pending || S.live.has(s.uid) || (draft?.cli?.instance?.running === true && !!takenOver(s.uid));
  const tmux = pending || S.liveTmux.has(s.uid), frozen = sessionFrozen(s.uid);
  const attention = !frozen && active ? sessionInputAttention(s.uid) : '';
  const turn = frozen || pending ? '' : sessionTurn(s.uid);
  return SessionDockSidebar.Presentation.statusView({running: active, count: unread.count, pending, tmux, frozen, attention, turn,
    turnLabel: turnLabel(turn), attentionLabel: inputAttentionLabel(attention)});
}
function sidebarAgentRunning(uid, agent) {
  const item = (indexedSessions().byUid.get(uid)?.agent_items || []).find(item => item.id === agent.id);
  return !!item && agentRunning(uid, item);
}
const pendingSidebarLive = new WeakMap();
function sidebarPendingRunning(s) {
  // New receipt rows start live before term/list catches up. Subsequent live
  // paints follow that list, as the original row class and status marker did.
  return !!s.tmuxName && (pendingSidebarLive.get(s) ?? !s.stale);
}
function sidebarPresentationState() {
  return {view: S.view, selected: S.sel, agent: S.agent, picking: S.picking, live: S.live, liveTmux: S.liveTmux,
    picked: pickedSessions, starBusy: S.starBusy, nestAttachUids: S.nestAttachUids, searching: !!S.term};
}
function sidebarPresentationHelpers() {
  return {pickable: sessionPickable, snippet: sidebarRowSnippet, highlight: hl, snippetHtml: sidebarSnippet,
    itemMeta, agentMeta, agentRunning: sidebarAgentRunning, pendingRunning: sidebarPendingRunning, status: sidebarStatus, source: source => SOURCES[source], path: timelinePath,
    nodeColor, shortCwd, nodeDirectory, groupClosed: sidebarGroupClosed, mainMatches: sidebarMainMatches,
    groupsAvailable: !!globalThis.SessionDockGroups?.available, groupContains: name => globalThis.SessionDockGroups.contains(name)};
}
function sidebarRowView(row) {return SessionDockSidebar.Presentation.rowView(row, sidebarPresentationState(), sidebarPresentationHelpers());}
function sidebarGroupView(key, rows, summary = undefined) {return SessionDockSidebar.Presentation.groupView(key, rows, summary, sidebarPresentationState(), sidebarPresentationHelpers());}
function refreshSidebarRows(uid = null) {
  if (!sidebarVueMounted || sidebarTextSelectionProtected()) { sidebarRenderDeferred = true; return; }
  SessionDockSidebar.repaintRows(sidebarRowView, uid || undefined);
}
function stampSidebarGroups() {
  for (const view of SessionDockSidebar.currentGroups()) {
    const group = [...$('#side').children].find(node => node.dataset.key === view.key);
    if (!group) continue;
    group._rows = view.rows.map(row => row.row); group._pickUids = view.pickUids;
    group.querySelector('.ghead')._pickLabel = view.label;
  }
}
function renderSide(suppliedList = null) {
  if (sidebarTextSelectionProtected()) {sidebarRenderDeferred = true; return;}
  sidebarRenderDeferred = false; ensureSidebarVue(); renderSessionCounts();
  const side = $('#side'), top = side.scrollTop, list = suppliedList || visible();
  side._sessionUids = new Set(list.map(row => row.uid)); paintSearchMode(list); syncPickedSessions(); renderPickBar();
  const groups = groupBy(list, {skipClosed: true});
  const empty = !list.length && !(S.view === 'group' && globalThis.SessionDockGroups?.available);
  side._nestTree = empty ? null : {children: groups.children, sessions: S.sessions, results: S.results, context: sidebarNestContext()};
  SessionDockSidebar.update({groups: empty ? [] : groups.map(([key, rows, summary]) => sidebarGroupView(key, rows, summary)),
    empty: !empty ? '' : S.term ? '当前搜索无匹配会话' : S.activeOnly ? (S.results ? '没有活动的匹配会话' : '没有活动会话') : (S.results ? '没有匹配的会话' : '没有会话'),
    searching: !!S.term, picking: S.picking, attaching: !!S.nestAttach,
    createGroup: S.view === 'group' && !!globalThis.SessionDockGroups?.available, groupMode: S.view === 'group',
    groupBusy: !!globalThis.SessionDockGroups?.busy, groupEditing: !!globalThis.SessionDockGroups?.editing});
  stampSidebarGroups(); fitTimelineDirectories(); side.scrollTop = top;
}


SessionDockSearch.configure({
  query: () => $('#q').value,
  read: () => ({term: S.term, opts: S.opts, results: S.results, off: S.off,
    cur: S.cur, markCapped: S.markCapped, autoOpen: S.autoOpen}),
  write: patch => Object.assign(S, patch),
  clearFolds: () => { S.searchClosed.clear(); S.searchNestClosed.clear(); },
  renderSide: () => renderSide(), showSessionCount: () => showSessionCount(),
  persistOptions: opts => store.set('opts', opts),
  allowsSearch: () => SessionDockCapabilities.allows('search'), appUrl,
  hub: HUB_MODE, sources: Object.keys(SOURCES), nodes: () => Nodes.list,
  selectedNodeIds, clearNodeErrors: () => { Nodes.errors.delete('search'); renderNodes(); },
  applyNodeState: data => applyNodeState(data, 'search'),
  count: rows => sidebarMatchCount(rows), escape: esc,
  showMatches: rows => showSearchMatches(rows),
});
function searchTerms(...args) { return SessionDockSearch.searchTerms(...args); }
function literalSource(...args) { return SessionDockSearch.literalSource(...args); }
function reTerm(...args) { return SessionDockSearch.reTerm(...args); }
function resetRegexSearch(...args) { return SessionDockSearch.resetRegexSearch(...args); }
function regexMatches(...args) { return SessionDockSearch.regexMatches(...args); }
function regexCached(...args) { return SessionDockSearch.regexCached(...args); }
function matchesSearch(...args) { return SessionDockSearch.matchesSearch(...args); }
function hasTerm(...args) { return SessionDockSearch.hasTerm(...args); }
function hl(...args) { return SessionDockSearch.hl(...args); }
function sidebarSnippet(...args) { return SessionDockSearch.sidebarSnippet(...args); }
function markMatches(...args) { return SessionDockSearch.markMatches(...args); }
function markRegexMatches(...args) { return SessionDockSearch.markRegexMatches(...args); }
function jumpMark(...args) { return SessionDockSearch.jumpMark(...args); }
function updateMatchNav(...args) { return SessionDockSearch.updateMatchNav(...args); }

// ---------------------------------------------------------------- 详情
let inflight = null;

function ensureConsolePlaceholder(){if($('#a-term'))return;$('#detail').prepend(SessionDockSessionUi.consolePlaceholder());showConsoleToast('');}

function followContinuedSession(uid) {
  const seen = new Set();
  let cur = uid;
  while (cur && !seen.has(cur)) {
    seen.add(cur);
    const next = S.sessions.find(s => s.uid === cur)?.continued_in;
    if (!next || next === cur || !S.sessions.some(s => s.uid === next)) return cur;
    cur = next;
  }
  return cur;
}

// 首次打开时的瞬时失败（503、断网）：正文保留“读取失败”，按退避自动重试
// 三次（1.5/3/6 s），期间可手动重试；仍失败则停在可点击重试的提示上。
const openRetries = new Map();
function scheduleOpenRetry(uid, agent, ac) {
  if (SessionDockCapabilities.config.backend !== 'rust') return;
  const key = viewKey(uid, agent);
  const attempt = (openRetries.get(key) || 0) + 1;
  const box = $('#detail .empty');
  const retry = () => {
    if (S.sel !== uid || S.agent !== agent || inflight !== ac) return;
    openRetries.set(key, attempt);
    openSession(uid, agent);
  };
  if (box) {
    const button = document.createElement('button');
    button.type = 'button'; button.className = 'btn';
    button.textContent = '重试读取';
    button.onclick = retry;
    box.append(' ', button);
  }
  if (attempt <= 3) {
    const timer = setTimeout(retry, retryDelay(attempt - 1));
    if (box) box.append(document.createTextNode(` 正在自动重试（${attempt}/3）…`));
    ac.signal.addEventListener('abort', () => clearTimeout(timer), {once: true});
  } else openRetries.delete(key);
}

// 当前会话被 Codex 回退成隐藏的父会话后，对话页和控制台一起跟到最深的分支：
// 同一个 pty 仍在写，只是原生叶子换了。用户主动打开的父会话（父会话链、
// 显示父会话）不在此列。
async function followSelectedFork() {
  const current = S.sessions.find(s => s.uid === S.sel);
  if (!current || S.agent || !hiddenForkParent(current)) return false;
  // 手机停在列表页时不把人拽进详情；父会话已从列表消失，点开分支即可。
  if (MOBILE.matches && store.get('mobilePage', 'list') !== 'detail') return false;
  const leaf = forkLeaf(current);
  if (!leaf || leaf.uid === S.sel) return false;
  browserAuditEvent('session.fork_followed', {from_uid: S.sel}, null, {uid: leaf.uid});
  if (typeof migrateComposerDraft === 'function') migrateComposerDraft(S.sel, leaf.uid);
  if (typeof T !== 'undefined' && T.uid === S.sel) T.uid = leaf.uid;
  await openSession(leaf.uid, null, {exact: true});
  return true;
}

/** One shareable route for sidebar navigation and external research links. */
function sessionUrl(uid, agent = null) {
  const row = S.sessions.find(s => s.uid === uid);
  if (!row?.sid) return null;
  const url = new URL(location.href);
  url.searchParams.set('sid', `${row.source}:${row.sid}${agent ? '/agent:' + agent : ''}`);
  if (row.node_id) url.searchParams.set('node', row.node_id);
  else url.searchParams.delete('node');
  return url;
}
function updateSessionUrl(uid, agent, mode) {
  const url = sessionUrl(uid, agent);
  if (!url || mode === 'none' || url.href === location.href) return;
  const method = mode === 'replace' || !new URL(location.href).searchParams.has('sid') ? 'replaceState' : 'pushState';
  history[method](history.state, '', url);
}
function revealSessionInSidebar(uid, agent) {
  const target = agent ? $('#side').querySelector(`.item[data-owner="${CSS.escape(uid)}"][data-agent="${CSS.escape(agent)}"]`)
    : $('#side').querySelector(`.item[data-uid="${CSS.escape(uid)}"]`);
  // Selecting an existing row opens its conversation, not its descendants.
  // In particular, do not clear a fold the user set with this row's caret.
  if (target) {
    target.scrollIntoView({block: 'nearest'});
    return;
  }
  const row = S.sessions.find(s => s.uid === uid);
  if (!row) return;
  let changed = false;
  if (!visible().some(s => s.uid === uid)) {
    if (S.term || S.results) cancelSearch(true);
    if (S.off.delete(row.source)) store.set('off', [...S.off]);
    if (HUB_MODE && Nodes.off.delete(row.node_id)) store.set('nodesOff', [...Nodes.off]);
    if (S.activeOnly) { S.activeOnly = false; store.set('activeOnly', false); }
    changed = true;
  }
  // 子代理行两种模式都在，深链不再替用户打开分层开关；分层模式才需要沿发起链展开祖先。
  const byKey = new Map(S.sessions.map(s => [spawnKey(s.node_id, s.source, s.sid), s]));
  const seen = new Set();
  for (let current = row; current && !seen.has(current.uid);
       current = S.nest ? nestParentOf(current, byKey) : null) {
    seen.add(current.uid);
    // A hidden link target needs its ancestors, but not its own children.
    if ((current.uid !== uid || agent) && sidebarNestFolds().delete(current.uid)) changed = true;
  }
  if (changed && !S.term) store.set('nestClosed', [...S.nestClosed]);
  const groups = groupBy(visible(), {skipClosed: true});
  const contains = session => session.uid === uid
    || (groups.children.get(session.uid) || []).some(contains);
  for (const [key, rows, summary] of groups) {
    const found = summary ? summary.roots.some(contains) : rows.some(r => r.s.uid === uid);
    if (found && sidebarGroupFolds().delete(key)) {
      if (!S.term) store.set('closed', [...S.closed]);
      changed = true;
    }
  }
  if (changed) { renderView(); renderChips(); if (HUB_MODE) renderNodes(); renderSide(); }
  const revealed = agent ? $('#side').querySelector(`.item[data-owner="${CSS.escape(uid)}"][data-agent="${CSS.escape(agent)}"]`)
    : $('#side').querySelector(`.item[data-uid="${CSS.escape(uid)}"]`);
  revealed?.scrollIntoView({block: 'nearest'});
}

/** `follow` continues the page already on screen (a new launch reaching its
 *  native history): keep the composer and its focus, and leave the launch
 *  stage up until the conversation replaces it instead of flashing a spinner. */
async function openSession(uid, agent = null, {exact = false, historyMode = 'push', follow = false} = {}) {
  const selectedAgent = agent || null;
  if (!selectedAgent) uid = followContinuedSession(uid);
  if (!selectedAgent && !exact && hiddenForkParent(S.sessions.find(s => s.uid === uid))) {
    uid = forkLeafUid(uid);
  }
  browserAuditEvent('session.opened', {agent: selectedAgent || '', cached: cache.has(viewKey(uid, selectedAgent))},
    null, {uid});
  showMobileDetail();
  inflight?.abort();            // 连点列表时, 放弃上一个还没回来的请求
  const ac = inflight = new AbortController();
  closeWatch();
  ++renderSeq;                 // Cancel detached render batches before the next fetch completes.
  syntaxQueue.clear();
  formulaRoots.clear();
  if (typeof T !== 'undefined') {
    if (T.uid && (T.uid !== uid || selectedAgent)) closeTermPane(true);
    if (!follow) SessionDockComposer.operations().hide();    // 先收起, 渲染完再按新会话的状态决定
  }
  S.sel = uid;
  S.agent = selectedAgent;
  syncSessionStopNotice();
  clearUnread(uid);
  store.set('sel', uid);
  store.set('agent', S.agent ? { uid, id: S.agent } : null);
  revealSessionInSidebar(uid, selectedAgent);
  paintSidebarSelection(uid, selectedAgent);
  updateSessionUrl(uid, selectedAgent, historyMode);

  const key = viewKey(uid, selectedAgent);
  const hit = cacheGet(key);
  if (hit) {
    // 先把缓存立即画出来，但暂不占一个长期 SSE 连接。补齐缓存游标之后再
    // 建 watch，避免 HTTP/1 连接池紧张时增量 fetch 永远排在 EventSource 后。
    await renderSession(hit.meta, hit.msgs, hit.activity, { startWatch: false });
    if (S.sel === uid && S.agent === selectedAgent) {
      await syncSession(uid, selectedAgent);
      if (S.sel === uid && S.agent === selectedAgent) watchSession(uid, selectedAgent);
    }
    return;
  }

  if (!follow) {
    $('#detail').innerHTML = '<div class="spin">正在读取会话…</div>';
    ensureConsolePlaceholder();
    auditDetailRendered('loading');
  }
  progress(0, 0, '下载');
  let res;
  try {
    res = await fetchMessages(uid, {
      agent: selectedAgent, signal: ac.signal,
      windowed: true,
      onProgress: (a, b, detail) => progress(a, b, '下载', detail),
    });
  } catch (e) {
    if (e.name === 'AbortError') return;      // 已经切到别的会话了
    if (SessionDockCapabilities.config.backend === 'rust'
        && (S.sel !== uid || S.agent !== selectedAgent)) return;
    progressDone();
    $('#detail').innerHTML = `<div class="empty">读取失败: ${esc(e.message)}</div>`;
    ensureConsolePlaceholder();
    if (!reportReadFailure(uid, selectedAgent, e)) scheduleOpenRetry(uid, selectedAgent, ac);
    auditDetailRendered('load-failed', {error: String(e.message || e).slice(0, 300)});
    return;
  }
  if (S.sel !== uid || S.agent !== selectedAgent) return; // 期间切了别的视图
  const { data, bytes } = res;
  if (SessionDockCapabilities.config.backend === 'rust') { migrationReadFailures.delete(key); openRetries.delete(key); }
  cachePut(key, { meta: data.meta, msgs: data.messages, version: data.version,
                  end: data.end, anchor: data.anchor, activity: data.activity, bytes,
                  prompt: data.prompt || null, cli: data.cli ?? null,
                  total: data.message_total, partial: data.partial || null });
  S.cursors.set(key, {end: data.end, head: data.version.head, anchor: data.anchor});
  await renderSession(data.meta, data.messages, data.activity);
}

// ---- 保持贴底 ----
// 只要用户没有主动往上翻, 消息区就一直停在最新一条: 新消息追加、消息展开/折叠、
// 图片或表格撑高、窗口缩放, 都要跟着走。
const BOTTOM_SLACK = 48;
let _stick = true, _lastTop = 0, _lockUntil = 0, _selfScroll = false;

/** 窗口缩放会让浏览器自己调整滚动位置, 那一小段时间内的滚动不算用户意图。
 *  只在 window resize 时用 —— 别在内容变化时也锁, 否则锁会被不断续期,
 *  用户主动往上翻都会被忽略。 */
function lockStick(ms = 300) { _lockUntil = performance.now() + ms; }

function atBottom(box) {
  return box.scrollHeight - box.scrollTop - box.clientHeight <= BOTTOM_SLACK;
}

function stickBottom(box, force) {
  if (force) _stick = true;
  if (!_stick) return;
  _selfScroll = true;
  box.scrollTop = box.scrollHeight;
  // 必须读回来: 浏览器会把它夹到 maxScroll, 直接记 scrollHeight 的话
  // 下一次 scroll 事件会把这当成"用户往上翻了一大截", 跟随就断了
  _lastTop = box.scrollTop;
  requestAnimationFrame(() => { _selfScroll = false; });
}

/** 展开/收起大块内容时把触发控件钉在原来的视口坐标。用户点开过程是在看
 *  这一段，不再属于“持续跟随最底部”；否则 ResizeObserver 会把按钮直接
 *  推出屏幕。补两帧可覆盖语法高亮等紧随其后的同步布局变化。 */
function mutateKeepingMessageAnchor(anchor, mutate) {
  const box = $('#msgs');
  if (!box || !box.contains(anchor)) return mutate();
  const top = anchor.getBoundingClientRect().top;
  _stick = false;
  _lastTop = box.scrollTop;
  const restore = () => {
    if (!anchor.isConnected || box !== $('#msgs')) return;
    const delta = anchor.getBoundingClientRect().top - top;
    if (Math.abs(delta) < .5) return;
    _selfScroll = true;
    box.scrollTop += delta;
    _lastTop = box.scrollTop;
    requestAnimationFrame(() => { _selfScroll = false; });
  };
  const result = mutate();
  restore();
  requestAnimationFrame(() => {
    restore();
    requestAnimationFrame(restore);
  });
  return result;
}

function jumpWithinConversation(target, block = 'center') {
  const box = $('#msgs');
  if (!target || !box?.contains(target)) return;
  _stick = false;
  _lastTop = box.scrollTop;
  target.scrollIntoView({block, behavior: 'smooth'});
}

function disposeMessageObservers(box) {
  box?._ro?.disconnect();
  box?._mo?.disconnect();
}

function watchBottom(box) {
  _stick = true;
  _lastTop = box.scrollTop;
  // 按滚动"方向"判断意图, 而不是按当前是否贴底 —— 窗口缩小、内容展开都会让
  // "是否贴底"瞬间变假, 那不是用户想离开底部。
  // 直接听用户的动作来判断"想离开底部"。不能只靠 scroll 事件推方向:
  // 布局重排、程序自身的滚动都会产生 scroll, 混在一起分不清谁是谁。
  const leave = () => { _stick = false; };
  box.addEventListener('wheel', e => { if (e.deltaY < 0) leave(); }, { passive: true });
  box.addEventListener('touchmove', leave, { passive: true });
  box.addEventListener('keydown', e => {
    if (['PageUp', 'ArrowUp', 'Home'].includes(e.key)) leave();
  });
  box.addEventListener('scroll', () => {
    const top = box.scrollTop;
    // 拖滚动条没有 wheel 事件, 靠方向补一手 (排除程序自己滚的那些)
    if (!_selfScroll && performance.now() >= _lockUntil && top < _lastTop - 2) _stick = false;
    else if (atBottom(box)) {
      _stick = true;                               // 回到底部就恢复跟随
      if (box._turnSealPending) queueMicrotask(() => flushPendingTurnSeal(box));
    }
    _lastTop = top;
  });
  // 内容高度变化 (展开消息、渲染完成、字体加载…) 时跟随
  if (window.ResizeObserver) {
    disposeMessageObservers(box);
    const ro = new ResizeObserver(() => settle(box));
    ro.observe(box);
    // 普通消息只盯底部 30 条，避免上万节点的观察成本；但图片、公式、表格即使
    // 位于很早的消息里，加载或窄屏重排也会改变总高度，必须额外观察。
    const rich = new Set(), ordinary = new Set();
    const observeRich = root => {
      const observe = node => { rich.add(node); ro.observe(node); };
      if (root.matches?.('img, .katex, .tw')) observe(root);
      root.querySelectorAll?.('img, .katex, .tw').forEach(observe);
    };
    const syncTail = () => {
      const tail = new Set();
      for (let node = box.lastElementChild; node && tail.size < 30; node = node.previousElementSibling) tail.add(node);
      for (const node of ordinary) if (!tail.has(node)) { ordinary.delete(node); if (!rich.has(node)) ro.unobserve(node); }
      for (const node of tail) if (!ordinary.has(node)) { ordinary.add(node); ro.observe(node); }
      for (const node of rich) if (!box.contains(node)) { rich.delete(node); ro.unobserve(node); }
    };
    syncTail();
    observeRich(box);
    box._ro = ro;
    const mo = new MutationObserver(ms => {
      for (const m of ms) for (const n of m.addedNodes) {
        if (n.nodeType === 1) observeRich(n);
      }
      syncTail();
      settle(box);
    });
    mo.observe(box, { childList: true, subtree: true });
    box._mo = mo;
  }
}

/** 布局要好几帧才稳: 窗口变窄会让消息重新折行, scrollHeight 一路涨,
 *  只修一次会差一截。补几帧, 直到不再变化。 */
let _settleTick = 0;

function settle(box) {
  if (!_stick) return;
  stickBottom(box);
  if (_settleTick) return;                   // 已经有补偿在跑, 别叠加
  let n = 0, last = -1;
  const tick = () => {
    _settleTick = 0;
    // 布局稳定(高度不再变)就收手 —— 一直空转的话 _selfScroll 长期为真,
    // 用户主动往上翻会被当成程序自己滚的而忽略掉
    if (!_stick || n++ > 10 || box !== $('#msgs') || box.scrollHeight === last) return;
    last = box.scrollHeight;
    stickBottom(box);
    _settleTick = requestAnimationFrame(tick);
  };
  _settleTick = requestAnimationFrame(tick);
}

addEventListener('resize', () => {
  const box = $('#msgs');
  if (!box) return;
  lockStick();
  settle(box);
  setTimeout(() => settle(box), 160);      // 兜住 resize 之后的异步重排
  setTimeout(() => settle(box), 400);
});

/** 整份渲染。消息可能上万条, 分批交还主线程, 否则页面会卡住不动。 */
let renderSeq = 0;



function historyPagesEnabled() {
  return SessionDockCapabilities.config.backend === 'rust'
    && SessionDockCapabilities.config.history_pages === true;
}

const historyPageRequests = SessionDockConversation.requests.historyPageRequests;
// 点一次“加载中间 N 条”后自动连续翻页直到缺口填满（分页取但不停）。
// 按钮显示进度，再点一次中止。用 let 是为了浏览器 E2E
// 能关掉连续翻页，逐页检验竞争。
let HISTORY_PAGE_CHAIN = true;

function currentHistoryPage(request) {
  return S.sel === request.uid && S.agent === request.agent
    && inflight === request.viewRequest && cache.get(request.key) === request.entry
    && request.entry.partial?.cursor === request.cursor;
}

const HISTORY_PAGE_MAX_EVENTS = 10000;   // 服务端 SESSIONDOCK_HISTORY_PAGE_EVENTS 的上限

const validateHistoryPage = SessionDockConversation.pages.validateHistoryPage;

async function fetchHistoryPage(uid, agent, cursor, signal, partial) {
  const query = new URLSearchParams({cursor});
  if (agent) query.set('agent', agent);
  if (partial?.resume) {
    query.set('resume', JSON.stringify(partial.resume));
    query.set('next', partial.head);
  }
  const url = `api/messages/${encodeURIComponent(uid)}/page`;
  const traceId = globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  browserAuditEvent('http.request.started', {url, method: 'GET', agent: agent || '',
    page_start: partial?.head, page_remaining: partial?.omitted, resumable: !!partial?.resume}, null, {uid, traceId});
  let response;
  try {
    response = await fetch(appUrl(`${url}?${query}`), {signal, cache: 'no-store',
      headers: {'X-SessionDock-Trace': traceId, 'X-SessionDock-Page': AUDIT_PAGE_ID,
        'X-SessionDock-Build': BUILD_ID}});
  } catch (error) {
    browserAuditEvent('http.request.failed', {url, error: String(error?.name || error)},
      null, {uid, traceId, severity: 'warning'});
    throw error;
  }
  browserAuditEvent('http.response.received', {url, status: response.status, ok: response.ok},
    null, {uid, traceId, severity: response.ok ? 'info' : 'warning'});
  if (!response.ok) {
    const detail = await response.json().catch(() => null);
    const error = new Error(detail?.error || `HTTP ${response.status}`);
    error.status = response.status;
    throw error;
  }
  const reader = response.body.getReader(), chunks = [];
  let bytes = 0;
  try {
    for (;;) {
      const {done, value} = await reader.read();
      if (done) break;
      bytes += value.length;
      if (bytes > 8 * 1024 * 1024) throw new Error('历史分页响应超过浏览器读取预算。');
      chunks.push(value);
    }
  } catch (error) { await reader.cancel().catch(() => {}); throw error; }
  const buffer = new Uint8Array(bytes);
  let offset = 0;
  for (const chunk of chunks) { buffer.set(chunk, offset); offset += chunk.length; }
  return {data: JSON.parse(new TextDecoder().decode(buffer)), bytes};
}

function historyPageFailure(request, button, error) {
  if (!currentHistoryPage(request)) return;
  SessionDockConversation.gapState({gapError:`未改变当前历史快照。${error.message || '历史分页读取失败。'}`,gapDisabled:false,gapLabel:'重试加载这一页',gapReloadBusy:false});
}

function restoreHistoryPageScroll(top, scrollTop, entry) {
  const box = $('#msgs');
  if (!box) return;
  const restore = () => {
    if (box !== $('#msgs') || cache.get(viewKey(S.sel, S.agent)) !== entry) return;
    _stick = false;
    _selfScroll = true;
    const gap = box.querySelector('.history-gap');
    box.scrollTop = gap ? box.scrollTop + gap.getBoundingClientRect().top - top : scrollTop;
    _lastTop = box.scrollTop;
    requestAnimationFrame(() => { _selfScroll = false; });
  };
  restore();
  requestAnimationFrame(() => { restore(); requestAnimationFrame(restore); });
}

async function loadHistoryPage(uid, agent, button) {
  agent = agent || null;
  const key = viewKey(uid, agent), entry = cache.get(key);
  if (!historyPagesEnabled() || !entry?.partial) return;
  const previous = historyPageRequests.get(key);
  if (previous && currentHistoryPage(previous)) {
    // 连续翻页进行中再点一次 = 中止；已取到的页保留。
    if (previous.chain && previous.ac) { previous.aborted = true; previous.ac.abort(); }
    return;
  }
  if (previous) { previous.ac?.abort(); historyPageRequests.delete(key); }
  const request = {key, uid, agent, entry, cursor: entry.partial.cursor, viewRequest: inflight,
    chain: HISTORY_PAGE_CHAIN, aborted: false};
  if (!currentHistoryPage(request)) return;
  const ac = new AbortController();
  request.ac = ac;
  let timer = setTimeout(() => ac.abort(), SYNC_STALL_MS);
  historyPageRequests.set(key, request);
  const target = Number(entry.partial.omitted || 0);
  let loaded = 0;
  const gap = button.parentElement, box = $('#msgs');
  const top = gap.getBoundingClientRect().top, scrollTop = box?.scrollTop || 0;
  // 连续翻页时按钮保持可点（用于中止），并显示进度。
  SessionDockConversation.gapState({gapDisabled:!request.chain,gapError:'',gapLabel:request.chain ? `正在加载历史… 0 / ${target.toLocaleString()} 条 · 点击中止` : '正在读取这一页…'});
  let changed = false, failure = null;
  try {
    for (;;) {
      if (!/^[0-9a-f]{32}$/.test(request.cursor || '')) throw new Error('历史分页凭据缺失，请重新载入当前历史。');
      const {data, bytes} = await fetchHistoryPage(uid, agent, request.cursor, ac.signal, entry.partial);
      if (!currentHistoryPage(request)) return;
      const page = validateHistoryPage(data, entry.partial, request.cursor);
      // Ordinary SSE appends may have advanced this same entry while HTTP was
      // pending. Keep that tail and every live cursor field exactly as observed.
      SessionDockConversation.pages.insertHistoryPage(entry,data,page,bytes);
      changed = true;
      loaded += data.messages.length;
      request.cursor = entry.partial?.cursor || null;
      if (!request.chain || !entry.partial) break;
      if (button.isConnected) SessionDockConversation.gapState({gapLabel:`正在加载历史… ${loaded.toLocaleString()} / ${target.toLocaleString()} 条 · 点击中止`});
      clearTimeout(timer);
      timer = setTimeout(() => ac.abort(), SYNC_STALL_MS);
    }
  } catch (error) {
    // 用户中止连续翻页：已取到的页保留，按钮回到剩余数；其余失败照常提示。
    if (!(error?.name === 'AbortError' && request.aborted)) failure = error;
  } finally {
    clearTimeout(timer);
    if (historyPageRequests.get(key) === request) historyPageRequests.delete(key);
  }
  if (changed && currentHistoryPageEntry(request)) {
    trimCache();
    await renderSession(entry.meta, entry.msgs, entry.activity, {startWatch: false, historyPageEntry: entry});
    if (S.sel === uid && S.agent === agent && cache.get(key) === entry && inflight === request.viewRequest) {
      restoreHistoryPageScroll(top, scrollTop, entry);
      if (failure) historyPageFailure(request, $('#msgs .history-gap-load'), failure);
    }
  } else if (failure) historyPageFailure(request, button, failure);
}

/** 页已取到、只等渲染：视图与缓存条目仍是发起时的那份即可（游标已推进）。 */
function currentHistoryPageEntry(request) {
  return S.sel === request.uid && S.agent === request.agent
    && inflight === request.viewRequest && cache.get(request.key) === request.entry;
}

async function reloadHistoryWindow(uid, agent, button) {
  agent = agent || null;
  const key = viewKey(uid, agent), entry = cache.get(key), viewRequest = inflight;
  if (!historyPagesEnabled() || !entry?.partial || S.sel !== uid || S.agent !== agent || historyPageRequests.has(key)) return;
  const request = {key, uid, agent, entry, cursor: entry?.partial?.cursor, viewRequest};
  // Unlike a gap page, a window reset replaces the complete cached snapshot.
  // An SSE update accepted after this request started must never be rolled back.
  const observed = {msgs: entry.msgs, version: entry.version, end: entry.end, anchor: entry.anchor,
    activity: entry.activity, meta: entry.meta, prompt: entry.prompt};
  const ac = new AbortController(), timer = setTimeout(() => ac.abort(), SYNC_STALL_MS);
  request.ac = ac;
  historyPageRequests.set(key, request);
  SessionDockConversation.gapState({gapReloadBusy:true});
  try {
    const {data, bytes} = await fetchMessages(uid, {agent, windowed: true, signal: ac.signal});
    if (!currentHistoryPage(request)) return;
    if (entry.msgs !== observed.msgs || entry.version !== observed.version
        || entry.end !== observed.end || entry.anchor !== observed.anchor
        || entry.activity !== observed.activity || entry.meta !== observed.meta || entry.prompt !== observed.prompt) {
      throw new Error('实时历史已更新；已保留新内容，请再次重新载入当前历史。');
    }
    if (!data?.reset || !data.meta || !data.version || !Array.isArray(data.messages)) throw new Error('服务端没有返回有效的当前历史窗口。');
    await applyDiff(uid, data, bytes, agent);
  } catch (error) { historyPageFailure(request, button.parentElement.querySelector('.history-gap-load'), error); }
  finally {
    clearTimeout(timer);
    if (currentHistoryPageEntry(request)) SessionDockConversation.gapState({gapReloadBusy:false});
    if (historyPageRequests.get(key) === request) historyPageRequests.delete(key);
  }
}

async function loadFullHistory(uid, agent, button) {
  const key = viewKey(uid, agent);
  const old = cache.get(key);
  if (!old?.partial) return;
  inflight?.abort();
  const ac = inflight = new AbortController();
  closeWatch();
  SessionDockConversation.gapState({gapDisabled:true,gapLabel:'正在载入完整历史…'});
  progress(0, 0, '下载完整历史');
  try {
    const {data, bytes} = await fetchMessages(uid, {
      agent, signal: ac.signal,
      onProgress: (a, b, detail) => progress(a, b, '下载完整历史', detail),
    });
    cachePut(key, {meta: data.meta, msgs: data.messages, version: data.version,
                   end: data.end, anchor: data.anchor, activity: data.activity, bytes,
                   prompt: data.prompt || null, cli: data.cli ?? null,
                   total: data.message_total, partial: null});
    S.cursors.set(key, {end: data.end, head: data.version.head, anchor: data.anchor});
    if (S.sel === uid && S.agent === agent) {
      await renderSession(data.meta, data.messages, data.activity);
    }
  } catch (e) {
    if (e.name !== 'AbortError') {
      if (S.sel === uid && S.agent === agent) SessionDockConversation.gapState({gapDisabled:false,gapLabel:'载入失败，点击重试'});
    }
  } finally {
    progressDone();
    const current = cache.get(key);
    if (S.sel === uid && S.agent === agent && current?.partial && !_es) {
      watchSession(uid, agent);
    }
  }
}

async function renderSession(meta, msgs, activity = null, {startWatch = true, historyPageEntry = null} = {}) {
  const seq = ++renderSeq, uid = meta.uid, agent = meta.agent_id || null;
  if (S.sel !== uid || S.agent !== agent) return;
  const renderedLength = msgs.length, d = $('#detail');
  disposeMessageObservers($('#msgs'));
  d.innerHTML = '';
  const entry = cache.get(viewKey(uid, agent));
  // Explicit legacy header host; B6 replaces this builder independently.
  SessionDockConversation.mountHeader(d,meta,entryTotal(entry || {msgs}));
  layoutSessionHead(); renderTimelinePinNotice(meta);
  const box = SessionDockConversation.mount(d);
  auditDetailRendered('render', {messages:msgs.length,seq});
  S.cur = -1; S.autoOpen = 0; S.markCapped = false;
  SessionDockConversation.planning.configurePlanning(S.compactTurns);
  const partial = entry?.partial, split = partial ? Math.min(+partial.head || 0, msgs.length) : 0;
  const openTail = activity?.state === 'working';
  const tailComplete = (activity && !['working','waiting'].includes(activity.state)) || (!activity && !S.live.has(uid));
  const plan = partial ? [...planTurns(msgs.slice(0,split),{tailComplete:false,foldTail:true}),
    {gap:{uid,agent,omitted:partial.omitted}}, ...planTurns(msgs.slice(split),{openTail,tailComplete})]
    : planTurns(msgs,{openTail,tailComplete});
  // Preserve the renderer's cancellable batch yields before publication.
  for (let i = 0; i < plan.length; i += RENDER_BATCH) {
    SessionDockConversation.prepare(box,plan.slice(i,i + RENDER_BATCH),i === 0,{uid,agent});
    if (i + RENDER_BATCH < plan.length) {
      progress(i + RENDER_BATCH,plan.length,'渲染');
      await new Promise(r => setTimeout(r,0));
      if (seq !== renderSeq || S.sel !== uid || S.agent !== agent) return;
    }
  }
  SessionDockConversation.publishPrepared(box);
  const latest = cache.get(viewKey(uid,agent));
  // The authoritative cache may grow while either page or reset render yields.
  if (latest && (!historyPageEntry || latest === historyPageEntry)) {
    if (latest.msgs !== msgs || latest.msgs.length !== renderedLength)
      appendMessages(box,latest.msgs.slice(renderedLength),null,{openTail:latest.activity?.state === 'working'});
    activity = latest.activity;
    if (activity?.state !== 'working') sealTurnTail(box,latest,{defer:false});
  }
  scheduleSyntax(); renderConversationTail(activity,uid);
  stickBottom(box,true); watchBottom(box); progressDone();
  markMatches(box); updateMatchNav({jump:true});
  if (typeof renderComposer === 'function') {if (S.agent) SessionDockComposer.operations().hide();else renderComposer();}
  if (seq === renderSeq && S.sel === uid && S.agent === agent) {
    renderMigrationReadFailure(uid,agent);
    if (startWatch) watchSession(uid,agent);
    if (typeof restoreTermPane === 'function') restoreTermPane(uid,agent);
    scheduleBrowserSnapshot('rendered');
  }
}




function closeSessionActions(...args) {return SessionUiApp.closeSessionActions(...args);}



// 会话头任何宽度都只占一行，且不因折叠留白。一行上的重要程度：标题 → 操作按钮（按菜单顺序：
// 星标、折叠过程、搜索、冻结/恢复与报告、移动/复制、停止/删除）→ 元信息（消息数、大小、起止时间、机器、目录、来源、
// 模型、会话号、分支）。宽屏/中屏长标题让到标题行的 40%（不少于 8em）为止，窄屏标题不让位；
// 标题之后先按顺序平铺操作，全放下了再把元信息按顺序跟在标题后面（.dbrief）；从放不下的那一项起
// 后面的全部收进 ⋯ 菜单（放不下某个按钮时元信息也不放，免得次要的露着、重要的反而折了）；
// 菜单空了 ⋯ 不显示。
// 消息数会随新消息变宽，留一点余量免得刚好放下的一项被裁掉

// Keep the original controls as the single action/capability authority. The
// session toolbar gets lightweight entries that invoke those same controls.
function syncSessionGlobalActions(...args) {return SessionUiApp.syncSessionGlobalActions(...args);}

function layoutSessionHead(...args) {return SessionUiApp.layoutSessionHead(...args);}

// 排版结果按签名去重记 header.layout：档位、平铺了哪些操作、标题后放了几项元信息、
// 标题行有没有横向溢出（溢出就意味着右侧按钮会被 #right 裁掉）。
let headerLayoutSignature = '';
function auditHeaderLayout(heading, tier, actions) {
  if (!heading.isConnected) return;   // head() 构建时先量一次，挂上之后才是真实排版
  try {
    const title = heading.querySelector('.dtitle');
    const brief = heading.querySelector('.dbrief');
    const data = {
      tier,
      inline: [...actions.querySelectorAll('button')].filter(b => !b.hidden && b.offsetWidth)
        .map(b => b.id || (b.dataset.reportBug !== undefined ? 'report-bug' : b.className.split(' ')[0])),
      brief: brief && !brief.hidden ? brief.children.length : 0,
      menu_meta: heading.querySelector('#session-actions-menu .dmeta')?.children.length ?? null,
      title_overflow: title ? title.scrollWidth - title.clientWidth : null,
      width: title?.clientWidth ?? null, head_height: heading.offsetHeight,
    };
    const signature = JSON.stringify(data);
    if (signature === headerLayoutSignature) return;
    headerLayoutSignature = signature;
    browserAuditEvent('header.layout', data);
  } catch { /* 审计不能影响排版 */ }
}





// 拖分割线、开合左栏、改窗口都会改变详情区宽度，元信息随之在标题后和 ⋯ 之间进出


function closeHeaderMenu(restoreFocus = false) { SessionDockShell.closeHeaderMenu(restoreFocus); }
function layoutHeader() { SessionDockShell.layoutHeader(); }

/** 子代理是否仍在跑：服务端按 transcript 与父会话的停止通知判断，父进程不在则一律不算。 */
const agentRunning = (uid, item) => !!item.active && S.live.has(uid);

/** 主会话/子代理下拉的行：主会话固定在前，子代理按结束时间倒序，还在跑的没有结束时间、排最前并带绿点。 */
function sessionViewRows(...args) {return SessionUiApp.sessionViewRows(...args);}

function head(...args) {return SessionUiApp.head(...args);}

/* ---------- 回退父会话链 ---------- */
// Codex 回退会生成子会话，原会话默认从左栏隐藏。子会话标题栏给一个图标，
// 下拉列出整条父会话链，每一级可单独显示到左栏或再次隐藏。父会话本身
// 也给同一个图标，列出它的子分支：从链上进入隐藏的父会话后能原路回去。


function closeForkChainMenu(...args) {return SessionUiApp.closeForkChainMenu(...args);}

function renderForkChainMenu(...args) {return SessionUiApp.renderForkChainMenu(...args);}





function newAssignedLaunchFor(...args) {return SessionUiApp.newAssignedLaunchFor(...args);}

function unusedNewAssignedLaunch(...args) {return SessionUiApp.unusedNewAssignedLaunch(...args);}

function renderSessionFreeze(...args) {return SessionUiApp.renderSessionFreeze(...args);}

function renderSessionAction(...args) {return SessionUiApp.renderSessionAction(...args);}

// Rust `session_stop`: the server stops only a managed host instance (Ctrl-D,
// then the host's guarded stop); an unmanaged/external CLI is a typed refusal.
// Without process detection `S.live` only holds sessions this page launched or
// took over, so a listed managed instance also makes the session stoppable.

function sessionStoppable(...args) {return SessionUiApp.sessionStoppable(...args);}

// Keep the frozen scene inside the selected session pane, never in global floats.
function syncSessionFreezeOverlay(...args) {return SessionUiApp.syncSessionFreezeOverlay(...args);}
function syncSessionStopNotice(...args) {return SessionUiApp.syncSessionStopNotice(...args);}
function showSessionStopNotice(...args) {return SessionUiApp.showSessionStopNotice(...args);}


function requestSessionStop(...args) {return SessionUiApp.requestSessionStop(...args);}

function stopSession(...args) {return SessionUiApp.stopSession(...args);}

// Rust 回收站能力：文件进服务端显式配置的回收站目录。


// 运行状态未知不等于已退出：只有用户明确确认 CLI 已退出，才带 force 重试。
function confirmForceDelete(...args) {return SessionUiApp.confirmForceDelete(...args);}

function requestSessionDelete(...args) {return SessionUiApp.requestSessionDelete(...args);}

// OpenCode 会话在它自己的数据库里：直接删除（连同子会话），不进回收站。


function del(...args) {return SessionUiApp.del(...args);}

// 连续工具调用/输出合并成一个可折叠的组；正在增长的时间线尾段保持展开，
// 等后面出现普通对话或任务结束后再自动封口。
const {TOOL_ROLES, SEARCH_ROLES, TURN_START_ROLES, GROUP_MIN, MESSAGE_TIME_GAP_MS,
  MESSAGE_TIME_CADENCE_MS, isGroupableTool, pairTools, planMessages, baseMessageRole,
  isTurnStart, sameNativeTurn, isTurnAssistant, isFinalAssistant, isPassiveTurnTail,
  markInterruptedTurn, turnConclusion, visiblePlanSize, turnKey, planTurnSegment,
  planTurn, planTurns, messageTimeRange, OUT_LINES, OUT_CHARS, outputStats,
  outPreviewInfo, toolOutputPath, CHANGE_LABEL, diffKind, diffCodePath, diffCodeParts,
  diffSides, turnProcessSummary, formatDuration} = SessionDockConversation.planning;

function refreshMessageTimeDividers(box = $('#msgs')) {
  if (box) SessionDockConversation.refresh(box);
}
function appendMessages(box, msgs, before = null, options = {}) {
  return SessionDockConversation.append(box, msgs, before, options);
}
function sealToolTail(box) { return SessionDockConversation.sealTools(box); }
function sealTurnTail(box, entry, options = {}) {
  SessionDockConversation.planning.configurePlanning(S.compactTurns);
  return SessionDockConversation.seal(box, entry, options);
}
function flushPendingTurnSeal(box) {
  if (!box?._turnSealPending || box !== $('#msgs')) return;
  box._turnSealPending = false;
  const entry = cache.get(viewKey(S.sel, S.agent));
  if (entry) renderSession(entry.meta, entry.msgs, entry.activity, {startWatch:false});
}

function clearSyntaxPaint(node) {
  delete node.dataset.syntaxDone;
  delete node.dataset.detected;
  delete node.dataset.codeLanguage;
  delete node.dataset.syntaxLanguages;
  node.classList.remove('hljs');
  for (const cls of [...node.classList]) if (cls.startsWith('language-')) node.classList.remove(cls);
}

function paintToolOutputDiff(pre) {
  if (!(pre instanceof HTMLElement) || pre.querySelector(':scope > .tool-diff-line')) return;
  const lines = pre.textContent.split('\n');
  const first = lines.findIndex(line => line.startsWith('diff --git '));
  const unified = first >= 0 ? first : lines.findIndex((line, i) =>
    line.startsWith('--- ') && lines.slice(i + 1, i + 4).some(x => x.startsWith('+++ ')));
  if (unified < 0) return;
  const path = diffCodePath(lines.slice(unified));
  const fragment = document.createDocumentFragment();
  lines.forEach((line, i) => {
    const row = el('span', 'tool-diff-line');
    const kind = i >= unified ? diffKind(line) : '';
    if (kind) row.classList.add(kind);
    const parts = path && diffCodeParts(line, kind);
    if (parts) {
      row.classList.add('code');
      row.appendChild(el('i', '', parts.marker));
      const code = el('code', '', parts.source || ' ');
      code.dataset.codePath = path;
      row.appendChild(code);
    } else {
      row.textContent = line || ' ';
    }
    fragment.appendChild(row);
  });
  pre.replaceChildren(fragment);
}



SessionDockOverlays.mountMedia({appUrl, capabilities:SessionDockCapabilities, hub:HUB_MODE,
  viewKey, cache, historyPageRequests, stallMs:SYNC_STALL_MS, fetchMessages, applyDiff,
  getView: () => ({uid:S.sel, agent:S.agent, request:inflight}), contains: element => !!$('#msgs')?.contains(element)});
const {safeMediaSrc, lazyMediaEnabled, mediaContinuationEnabled, diagnoseMedia,
  reloadMediaSession, forgetMediaDiagnostic, captureView} = SessionDockOverlays;

// Scoped inline-media adapter renders Vue MediaError controls for Markdown images.
// Gallery images marked data-vue-media remain owned by B5 MediaImage.

function imageHtml(m, inline = false) {
  if (m?.error) {
    const reason = esc(String(m.error.message || '图片不可用'));
    return `<span class="media-error${inline ? ' inline' : ''}" role="status">图片不可用：${reason}</span>`;
  }
  const src = safeMediaSrc(m?.src);
  if (!src) return '';
  const alt = esc(m.alt || '图片');
  const w = Number.isFinite(+m.width) && +m.width > 0 ? ` width="${Math.round(+m.width)}"` : '';
  const h = Number.isFinite(+m.height) && +m.height > 0 ? ` height="${Math.round(+m.height)}"` : '';
  const lazy = lazyMediaEnabled() && m.lazy === true;
  const tracking = lazy ? ` data-media-lazy="true" data-media-path="${esc(m.src)}"` : '';
  const html = `<a class="media-link${inline ? ' inline' : ''}" href="${esc(src)}" target="_blank" rel="noopener noreferrer">
    <img loading="lazy" decoding="async" referrerpolicy="no-referrer" src="${esc(src)}" alt="${alt}"${w}${h}${tracking}>
    ${inline ? '' : `<span>${alt}</span>`}
  </a>`;
  return lazy ? `<span class="media-load${inline ? ' inline' : ''}">${html}</span>` : html;
}

// Rust-only per-message continuation: a message carries at most 16 typed
// images plus `media_more`. When it is absent, markup is unchanged.
function mediaMoreInfo(more) {
  if (!more || typeof more !== 'object' || Array.isArray(more)) return null;
  const {remaining, total, cursor} = more;
  if (!Number.isSafeInteger(remaining) || remaining <= 0 || !Number.isSafeInteger(total) || total < remaining) return null;
  if (cursor !== null && !(typeof cursor === 'string' && /^[0-9a-f]{32}$/.test(cursor))) return null;
  return {remaining, total, cursor};
}

function mediaMoreHtml(more) {
  const info = mediaContinuationEnabled() ? mediaMoreInfo(more) : null;
  if (!info) return '';
  const count = info.remaining.toLocaleString(), title = ` title="共 ${info.total.toLocaleString()} 张图片"`;
  return info.cursor
    ? `<button type="button" class="media-more" data-media-cursor="${info.cursor}"${title}>还有 ${count} 张图片，加载下一批</button>`
    : `<button type="button" class="media-more" disabled${title}>还有 ${count} 张图片暂不可加载</button>`;
}

function mediaGallery(items, more) {
  const html = (items || []).filter(x => x.gallery || !x.ref).map(x => imageHtml(x)).filter(Boolean);
  const pending = mediaMoreHtml(more);
  if (pending) html.push(pending);
  return html.length ? `<div class="media-gallery">${html.join('')}</div>` : '';
}

const mediaPageRequests = SessionDockConversation.requests.mediaPageRequests;

function currentMediaPage(request) {
  return S.sel === request.uid && S.agent === request.agent
    && inflight === request.viewRequest && cache.get(request.key) === request.entry
    && request.message.media_more?.cursor === request.cursor;
}

const validateMediaPage = SessionDockConversation.pages.validateMediaPage;

async function fetchMediaPage(uid, agent, cursor, signal) {
  const query = new URLSearchParams({cursor});
  if (agent) query.set('agent', agent);
  const response = await fetch(appUrl(`api/messages/${encodeURIComponent(uid)}/media-page?${query}`),
    {signal, cache: 'no-store'});
  if (!response.ok) {
    const detail = await response.json().catch(() => null);
    const error = new Error(detail?.error || `HTTP ${response.status}`);
    error.status = response.status;
    throw error;
  }
  const reader = response.body.getReader(), chunks = [];
  let bytes = 0;
  try {
    for (;;) {
      const {done, value} = await reader.read();
      if (done) break;
      bytes += value.length;
      if (bytes > 1024 * 1024) throw new Error('图片分页响应超过浏览器读取预算。');
      chunks.push(value);
    }
  } catch (error) { await reader.cancel().catch(() => {}); throw error; }
  const buffer = new Uint8Array(bytes);
  let offset = 0;
  for (const chunk of chunks) { buffer.set(chunk, offset); offset += chunk.length; }
  return {data: JSON.parse(new TextDecoder().decode(buffer)), bytes};
}

function mediaPageFailure(request, button, error) {
  if (!currentMediaPage(request)) return;
  SessionDockConversation.mediaState(request.cursor,{busy:false,error:`已加载的图片保持不变${error.status ? `（HTTP ${error.status}）` : ''}：${error.message || '图片分页读取失败。'}`});
}

async function loadMediaContinuation(uid, agent, cursor, button) {
  agent = agent || null;
  const key = viewKey(uid, agent), entry = cache.get(key);
  if (!mediaContinuationEnabled() || !entry?.msgs || !/^[0-9a-f]{32}$/.test(cursor || '')) return;
  const message = entry.msgs.find(m => m?.media_more?.cursor === cursor);
  if (!message || !mediaMoreInfo(message.media_more)) return;
  const previous = mediaPageRequests.get(cursor);
  if (previous && currentMediaPage(previous)) return;
  if (previous) { previous.ac?.abort(); mediaPageRequests.delete(cursor); }
  const request = {key, uid, agent, entry, message, cursor, viewRequest: inflight};
  if (!currentMediaPage(request)) return;
  const ac = new AbortController();
  request.ac = ac;
  const timer = setTimeout(() => ac.abort(), SYNC_STALL_MS);
  mediaPageRequests.set(cursor, request);
  SessionDockConversation.mediaState(cursor,{busy:true,error:''});
  try {
    const {data} = await fetchMediaPage(uid, agent, cursor, ac.signal);
    if (!currentMediaPage(request)) return;
    const page = validateMediaPage(data, message.media_more, cursor);
    // Only this message changes. Live cursor fields, message order and any
    // SSE tail accepted while HTTP was pending stay exactly as observed.
    message.media = [...(message.media || []), ...data.media];
    if (page.remaining) message.media_more = {remaining: page.remaining, total: page.total, cursor: page.next};
    else delete message.media_more;
    SessionDockConversation.mediaState(cursor, {busy:false,error:''});
    SessionDockConversation.updateMedia();
  } catch (error) { mediaPageFailure(request, button, error); }
  finally {
    clearTimeout(timer);
    if (mediaPageRequests.get(cursor) === request) mediaPageRequests.delete(cursor);
  }
}

document.addEventListener('click', event => {
  const button = event.target?.closest?.('.media-more');
  if (!button || !button.closest('[data-conversation-inner]') || button.disabled || !mediaContinuationEnabled() || !$('#msgs')?.contains(button)) return;
  event.preventDefault();
  loadMediaContinuation(S.sel, S.agent, button.dataset.mediaCursor, button);
});

let formulaLoading = null;
const formulaRoots = new Set();
function renderFormulae(root) {
  if (!/\$|\\[([]/.test(root.textContent || '')) return;
  if (typeof renderMathInElement !== 'function') {
    formulaRoots.add(root);
    if (!formulaLoading) {
      formulaLoading = Promise.all([
        SessionDockAssets.style('vendor/katex/katex.min.css'),
        SessionDockAssets.script('vendor/katex/katex.min.js'),
      ]).then(() => SessionDockAssets.script('vendor/katex/auto-render.min.js'))
        .then(() => {
          const roots = [...formulaRoots];
          formulaRoots.clear();
          for (const pending of roots) {
            if (pending.isConnected || pending.getRootNode().nodeType === Node.DOCUMENT_FRAGMENT_NODE)
              renderFormulae(pending);
          }
        }).catch(() => { formulaRoots.clear(); })
        .finally(() => { formulaLoading = null; });
    }
    return;
  }
  try {
    renderMathInElement(root, {
      delimiters: [
        { left: '$$', right: '$$', display: true },
        { left: '\\[', right: '\\]', display: true },
        { left: '\\(', right: '\\)', display: false },
        { left: '$', right: '$', display: false },
      ],
      ignoredTags: ['script', 'noscript', 'style', 'textarea', 'pre', 'code'],
      throwOnError: false,
      strict: 'ignore',
      trust: false,
    });
  } catch { /* 单个坏公式按原文保留，不能拖垮整条消息 */ }
}

const screenMenuTextDrafts = new Map();
function pendingHistoryQuestion(entry) {
  if (entry?.meta?.source !== 'codex' || entry?.activity?.state !== 'waiting') return null;
  const answered = new Set((entry.msgs || []).filter(m => ['answer','tool_result'].includes(m.role) && m.call_id).map(m => m.call_id));
  return [...(entry.msgs || [])].reverse().find(m => m.role === 'question' && m.call_id && !answered.has(m.call_id)) || null;
}
function pruneQuestionFormDrafts(uid, activeId = '') {
  const prefix = `${uid}\0`, keep = activeId ? `${prefix}${activeId}` : '';
  for (const key of questionFormDrafts.keys()) if (key.startsWith(prefix) && key !== keep) questionFormDrafts.delete(key);
}
function renderActivity(activity) {
  const box = $('#msgs'); if (!box) return;
  let visible = activity && ['working','waiting','aborted','failed'].includes(activity.state) ? activity : null;
  if (visible?.state === 'working') {
    const processStart = S.liveStarted.get(S.sel), at = Date.parse(visible.ts) / 1000;
    if (!S.live.has(S.sel) || (Number.isFinite(processStart) && Number.isFinite(at) && at < processStart - 2)) visible = null;
  }
  SessionDockConversation.tail(box, {activity:visible});
}
function renderTerminalThreadNotice(uid = S.sel) {
  const box = $('#msgs'); if (!box) return;
  SessionDockConversation.tail(box, {sideThread:!S.agent && uid === S.sel && sessiondockCli(uid)?.source === 'codex' && !!globalThis.codexSideThreadVisible?.(uid)});
}
function renderConversationTail(activity, uid = S.sel) {
  const box = $('#msgs'); if (!box) return;
  const entry = cache.get(viewKey(uid)), prompt = entry?.prompt;
  const nativeQuestion = prompt?.questions?.length ? null : pendingHistoryQuestion(entry);
  const activeQuestion = prompt?.questions?.length ? prompt : nativeQuestion;
  pruneQuestionFormDrafts(uid, activeQuestion && (activeQuestion.state || 'waiting') === 'waiting' ? String(activeQuestion.id || activeQuestion.call_id || '') : '');
  const question = prompt?.questions?.length ? {
    role:'question', call_id:prompt.id, questions:prompt.questions,
    text:prompt.questions.map(q => q.question).join('\n\n'), live:true,
    state:prompt.state, uid, ts:prompt.ts || prompt.created_at,
  } : nativeQuestion ? {...nativeQuestion, live:true, uid} : null;
  SessionDockConversation.tail(box, {uid,agent:S.agent,question,shadow:question?.call_id || '',activity:null});
  if (!question) renderActivity(activity);
  renderTerminalThreadNotice(uid);
  if (typeof syncComposerSendState === 'function') syncComposerSendState();
  if (typeof renderQueuedSends === 'function') renderQueuedSends(uid);
  scheduleBrowserSnapshot('conversation-tail');
}

const CLIP = 4000;
const AUTO_OPEN_MAX = 40;    // 最多自动展开这么多条命中消息, 其余只标记
const MARK_MAX = 3000;       // 单页高亮节点上限
const clipText = t => t.length > CLIP ? t.slice(0, CLIP) + '\n… (点下方按钮展开全文)' : t;

let syntaxWorker = null, syntaxBusy = false, syntaxFrame = null;
const syntaxQueue = new Set();
const syntaxGenerations = new WeakMap();

function scheduleSyntax() {
  if (syntaxBusy || syntaxFrame !== null || !syntaxQueue.size) return;
  syntaxFrame = requestAnimationFrame(() => {
    syntaxFrame = null;
    let code;
    const waiting = [];
    for (const candidate of syntaxQueue) {
      syntaxQueue.delete(candidate);
      if (candidate.isConnected && !candidate.dataset.syntaxDone) { code = candidate; break; }
      // Large histories yield while their nodes still live in a fragment.
      // Keep those jobs until publication, but discard abandoned render plans.
      if (!candidate.dataset.syntaxDone && syntaxGenerations.get(candidate) === renderSeq
          && candidate.getRootNode().nodeType === Node.DOCUMENT_FRAGMENT_NODE) waiting.push(candidate);
    }
    for (const candidate of waiting) syntaxQueue.add(candidate);
    if (!code) return;         // Fragment publication schedules us; do not spin every frame.
    syntaxBusy = true;
    const source = code.textContent;
    const language = code.dataset.codeLang || '', path = code.dataset.codePath || '';
    const kind = code.matches('code.tool-command') ? 'command'
      : code.matches('pre.tool-out') ? 'tool' : 'code';
    let timer;
    const finish = result => {
      clearTimeout(timer);
      // An expanded/replaced preview must never receive an older worker reply.
      if (code.isConnected && code.textContent === source
          && (code.dataset.codeLang || '') === language && (code.dataset.codePath || '') === path) {
        code.dataset.syntaxDone = '1';
        if (result?.html) {
          code.innerHTML = result.html;
          code.classList.add('hljs');
          if (result.language) code.classList.add(`language-${result.language}`);
          if (result.languages?.length) code.dataset.syntaxLanguages = result.languages.join(',');
          if (result.detected) code.dataset.detected = result.language;
          const host = code.tagName === 'PRE' ? code
            : (code.classList.contains('code-block') && code.parentElement?.tagName === 'PRE'
                ? code.parentElement : null);
          if (result.language && host) host.dataset.codeLanguage = result.language;
          // Syntax arrives after message search decoration now. Reapply the
          // current query to the new text nodes instead of losing its marks.
          if (S.term) { markMatches(code); updateMatchNav(); }
        }
      }
      syntaxBusy = false;
      scheduleSyntax();
    };
    const failed = () => {
      syntaxWorker?.terminate();
      syntaxWorker = null;
      finish(null);
    };
    try {
      syntaxWorker ||= new Worker(SessionDockAssets.url('syntax-worker.js'), {type: 'module'});
      syntaxWorker.onmessage = event => finish(event.data);
      syntaxWorker.onerror = failed;
      // Optional decoration must not hold up other blocks indefinitely. The
      // complete original text stays readable even when a grammar stalls.
      timer = setTimeout(failed, 5000);
      syntaxWorker.postMessage({source, kind, language, path});
    } catch { failed(); }
  });
}

function paintSyntax(root = document) {
  const select = selector => [
    ...(root.matches?.(selector) ? [root] : []),
    ...root.querySelectorAll(selector),
  ];
  const blocks = select('code.code-block:not([data-syntax-done])');
  const summaries = select('code.tool-command:not([data-syntax-done])');
  const tools = select('pre.tool-out:not([data-syntax-done])').filter(
    pre => !pre.querySelector(':scope > .tool-diff-line'));
  const diffLines = select(
    '.diff-line > code[data-code-path]:not([data-syntax-done]), '
    + '.tool-diff-line > code[data-code-path]:not([data-syntax-done])');
  const nodes = [...new Set([...blocks, ...summaries, ...tools, ...diffLines])];
  if (!nodes.length) return;
  for (const code of nodes) {
    syntaxQueue.add(code);
    syntaxGenerations.set(code, renderSeq);
  }
  scheduleSyntax();
}

// 轻量 markdown: 代码块 / 表格 / 列表 / 引用 / 标题 / 行内标记
function md(text, full, media = [], context = {}) {
  const source = full ? text : clipText(text);
  const lines = source.split('\n');
  const output = [], prose = [];
  const flush = () => {
    if (!prose.length) return;
    output.push(blocks(prose.join('\n'), media, context));
    prose.length = 0;
  };
  for (let i = 0; i < lines.length;) {
    // 只有独立行上的 Markdown 围栏才是代码块。旧的 split(/```/)
    // 会把句子里提到的 ```python 也当成开头，后半条消息全吞进 pre。
    const open = lines[i].match(/^\s{0,3}(`{3,}|~{3,})[^\S\n]*(.*)$/);
    if (!open || (open[1][0] === '`' && open[2].includes('`'))) {
      prose.push(lines[i++]);
      continue;
    }
    const marker = open[1][0], width = open[1].length;
    let close = i + 1;
    for (; close < lines.length; close++) {
      const found = lines[close].match(/^\s{0,3}(`+|~+)\s*$/);
      if (found && found[1][0] === marker && found[1].length >= width) break;
    }
    // 未闭合围栏只在“展开全文”预览恰好截断时按代码处理；
    // 原文本本身未闭合时保留原样，不再把整条消息误判为代码。
    if (close === lines.length && (full || text.length <= CLIP)) {
      prose.push(lines[i++]);
      continue;
    }
    flush();
    const info = open[2].trim();
    const language = info.match(/^[\w+.-]+/)?.[0] || '';
    const code = lines.slice(i + 1, close).join('\n');
    output.push(`<pre><code class="code-block" data-code-lang="${esc(language)}">${esc(code)}</code></pre>`);
    i = close < lines.length ? close + 1 : close;
  }
  flush();
  return output.join('') || '<p></p>';
}

const RE_LIST = /^\s*([-*+]|\d+[.)])\s+/;
const RE_HEAD = /^\s*(#{1,6})\s+(.*)$/;
const RE_QUOTE = /^\s*>\s?/;
const RE_HR = /^\s*([-*_])\s*(\1\s*){2,}$/;
// 表格分隔行: |---|:--:|  至少一个竖线和一串短横
const isSep = s => /^\s*\|?[\s:|-]+\|[\s:|-]*$/.test(s) && s.includes('-');
const cells = s => s.trim().replace(/^\||\|$/g, '').split('|').map(x => x.trim());
const isTable = (ls, i) => ls[i].includes('|') && i + 1 < ls.length && isSep(ls[i + 1]);

function blocks(src, media = [], context = {}) {
  const ls = src.split('\n');
  let out = '', i = 0;
  while (i < ls.length) {
    const line = ls[i];
    if (!line.trim()) { i++; continue; }

    if (isTable(ls, i)) {
      const head = cells(line);
      const align = cells(ls[i + 1]).map(c =>
        /^:-+:$/.test(c) ? 'center' : /-+:$/.test(c) ? 'right' : 'left');
      const at = j => `style="text-align:${align[j] || 'left'}"`;
      i += 2;
      const rows = [];
      while (i < ls.length && ls[i].trim() && ls[i].includes('|')) rows.push(cells(ls[i++]));
      out += `<div class="tw"><table><thead><tr>${head.map((c, j) => `<th ${at(j)}>${inline(c, media, context)}</th>`).join('')}</tr></thead>`
        + `<tbody>${rows.map(r => `<tr>${r.map((c, j) => `<td ${at(j)}>${inline(c, media, context)}</td>`).join('')}</tr>`).join('')}</tbody></table></div>`;
      continue;
    }

    const h = line.match(RE_HEAD);
    if (h) { out += `<h3 class="h${h[1].length}">${inline(h[2], media, context)}</h3>`; i++; continue; }

    if (RE_HR.test(line)) { out += '<hr>'; i++; continue; }

    if (RE_LIST.test(line)) {
      const tag = /^\s*\d/.test(line) ? 'ol' : 'ul';
      const items = [];
      while (i < ls.length && RE_LIST.test(ls[i])) {
        let item = ls[i++].replace(RE_LIST, '');
        while (i < ls.length && ls[i].trim() && !RE_LIST.test(ls[i]) && /^\s{2,}/.test(ls[i])) {
          item += '\n' + ls[i++].trim();     // 续行并入当前条目
        }
        items.push(`<li>${inline(item, media, context)}</li>`);
      }
      out += `<${tag}>${items.join('')}</${tag}>`;
      continue;
    }

    if (RE_QUOTE.test(line)) {
      const qs = [];
      while (i < ls.length && RE_QUOTE.test(ls[i])) qs.push(ls[i++].replace(RE_QUOTE, ''));
      out += `<blockquote>${inline(qs.join('\n'), media, context)}</blockquote>`;
      continue;
    }

    const para = [];
    while (i < ls.length && ls[i].trim() && !RE_LIST.test(ls[i]) && !RE_HEAD.test(ls[i])
           && !RE_QUOTE.test(ls[i]) && !RE_HR.test(ls[i]) && !isTable(ls, i)) {
      para.push(ls[i++]);
    }
    if (para.length) out += `<p>${inline(para.join('\n'), media, context)}</p>`;
    else i++;                                 // 兜底: 保证 i 一定前进
  }
  return out;
}

const RE_MD_IMAGE = /!\[([^\]]*)\]\(\s*(<[^>]+>|[^\s)]+)(?:\s+["'][^"']*["'])?\s*\)/g;
const RE_CODE_SPAN = /(^|[^`])(`+)(?!`)([^\n]*?)(?<!`)\2(?!`)/g;

const RE_REFERENCE = /(?<![A-Za-z0-9_@/\\:.-])(?:(?:https?:\/\/|www\.)[^\s<>"'`\u0000，。；、！？]+|[A-Za-z]:[/\\][^\s<>"'`\u0000，。；、！？()[\]{}]*|(?:~\/|\.\.?\/|\/|[A-Za-z0-9_.-]+\/)[^\s<>"'`\u0000，。；、！？()[\]{}]+|[A-Za-z0-9_-][A-Za-z0-9_.-]*\.[A-Za-z][A-Za-z0-9_-]*(?::\d+(?::\d+)?|#L\d+(?:C\d+)?)?)/gi;

const isWindowsDrivePath = path => /^[A-Za-z]:[/\\]/.test(path);
function fileParentDirectory(path) {
  const slash = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  // A drive root includes its separator: X: alone is drive-relative.
  return isWindowsDrivePath(path) && slash === 2 ? path.slice(0, 3) : path.slice(0, slash) || '/';
}

function trimReference(raw) {
  let ref = raw.replace(/[.,;:!?]+$/, '');
  for (const [left, right] of [['(', ')'], ['[', ']']]) {
    while (ref.endsWith(right) && ref.split(right).length > ref.split(left).length) ref = ref.slice(0, -1);
  }
  return ref;
}

function referenceLink(ref, label, context, explicit = false) {
  let href = '';
  if (/^(https?:\/\/|www\.)/i.test(ref)) {
    try {
      const url = new URL(/^www\./i.test(ref) ? 'https://' + ref : ref);
      if (!['http:', 'https:'].includes(url.protocol)) return '';
      href = url.href;
    } catch { return ''; }
  } else {
    // Browser file:// navigation cannot reach a remote node. Resolve only
    // references present in this session, through its authenticated API.
    const withoutLine = ref.replace(/(?::\d+(?::\d+)?|#L\d+(?:C\d+)?)$/, '');
    const windowsDrive = isWindowsDrivePath(withoutLine);
    if (!context.uid || (!windowsDrive && /^[a-z][a-z0-9+.-]*:/i.test(withoutLine)) || ref.startsWith('//')) return '';
    if (/^[A-Z0-9]+(?:\/[A-Z0-9]+)+$/.test(ref)) return '';
    // Rendering is lexical only. Resolve history, existence and ambiguity on
    // explicit navigation/menu actions, never once per render or SSE update.
    const filename = /^[^\s/<>"'`=;|{}\[\]]+\.[a-zA-Z][\w.-]*$/.test(withoutLine);
    const path = windowsDrive || /^(?:~\/|\.\.?\/|\/)[^\n]+$/.test(withoutLine)
      || (!/[\s<>"'`=;|{}\[\]]/.test(withoutLine) && withoutLine.includes('/')
          && (withoutLine.endsWith('/') || /^[^/]+\.[a-zA-Z][\w.-]*$/.test(withoutLine.split('/').pop())))
      || filename;
    if (!path && !explicit) return '';
    const query = new URLSearchParams({uid: context.uid, ref});
    if (context.agent) query.set('agent', context.agent);
    href = appUrl('/api/session/file') + '?' + query;
    const open = appUrl('file.html') + '?' + query + '&open=1';
    return `<a href="${esc(open)}" data-file-ref="${esc(ref)}" data-file-href="${esc(href)}" target="_blank" rel="noopener noreferrer">${label}</a>`;
  }
  return `<a href="${esc(href)}" data-reference-kind="web" target="_blank" rel="noopener noreferrer">${label}</a>`;
}

SessionDockOverlays.mountFiles({appUrl, allows:SessionDockCapabilities.allows,
  closeItemMenu, alert:appAlert});

function inline(s, media = [], context = {}) {
  s = String(s).replace(/\u0000/g, '');
  const codeSpans = [], codeLabels = [], codeText = [];
  s = s.replace(RE_CODE_SPAN, (_, prefix, _ticks, raw) => {
    // Markdown 代码跨度允许内容中出现更长的反引号串，例如用单反引号
    // 包住 ```python。先占位再处理图片/粗体，避免代码内容被二次解析。
    const content = raw.startsWith(' ') && raw.endsWith(' ') && /\S/.test(raw)
      ? raw.slice(1, -1) : raw;
    const code = `<code>${esc(content)}</code>`;
    codeText.push(content);
    codeLabels.push(code);
    codeSpans.push(code);
    return `${prefix}\u0000CODE${codeSpans.length - 1}\u0000`;
  });
  const images = [];
  s = s.replace(RE_MD_IMAGE, (_, alt, raw) => {
    const ref = raw.replace(/^<|>$/g, '');
    const found = media.find(x => x.ref === ref);
    const src = found?.src || ref;
    const html = imageHtml({ ...(found || {}), src, alt: alt || found?.alt || '图片' }, true);
    if (!html) return `[图片: ${alt || ref}]`;
    images.push(html);
    return `\u0000IMG${images.length - 1}\u0000`;
  });
  const links = [];
  const keepLink = html => {
    links.push(html);
    return `\u0000LINK${links.length - 1}\u0000`;
  };
  const emphasis = text => text
    .replace(/\*\*([^*\n]+)\*\*/g, '<b>$1</b>')
    .replace(/(^|[^*\w])\*([^*\n]+)\*(?!\w)/g, '$1<i>$2</i>');
  // Standard Markdown links display their label only. Keep the destination
  // in link metadata for navigation/menu actions, never append it to the label.
  s = s.replace(/\[([^\]\n]+)\]\(\s*(<[^>\n]+>|(?:[^\s()]|\([^\s()]*\))+)(?:\s+["']([^"']*)["'])?\s*\)/g,
    (_raw, label, target, title) => {
      const content = emphasis(esc(label)).replace(/\u0000CODE(\d+)\u0000/g,
        (_, i) => codeLabels[+i] || '');
      const ref = target.replace(/^<|>$/g, '');
      let html = referenceLink(ref, content, context, true) || content;
      if (title !== undefined && html.startsWith('<a ')) html = html.replace('<a ', `<a title="${esc(title)}" `);
      return keepLink(html);
    });
  const linkCandidate = (raw, offset, source) => {
    // Do not link a suffix of a scheme, identifier or email address.
    if (offset && /[\w@/\\:.-]/.test(source[offset - 1])) return raw;
    const ref = trimReference(raw);
    const html = referenceLink(ref, esc(ref), context);
    return html ? keepLink(html) + raw.slice(ref.length) : raw;
  };
  // Parentheses opt prose into linkification. Code spans are explicit
  // references too, including when they appear outside parentheses.
  const parenthesized = (part, explicit) => {
    // A complete target may itself contain balanced parentheses. Do not split
    // a URL or a filename such as report(final).pdf into several links.
    // Preserve every delimiter, space and optional Markdown title. Only wrap
    // the existing target substring; never synthesize a label or new text.
    const match = part.match(/^(\s*)(<([^<>\n]+)>|(?:[^\s()]|\([^\s()]*\))+)(\s+(?:["'][^"']*["'])\s*|\s*)$/);
    if (match && !match[2].includes('\u0000')) {
      const ref = match[3] || match[2];
      const html = referenceLink(ref, esc(ref), context, explicit);
      if (html) return match[1] + (match[3] ? '<' : '') + keepLink(html)
        + (match[3] ? '>' : '') + match[4];
    }
    return part.replace(/\u0000CODE(\d+)\u0000/g, (raw, i) => {
      const code = codeLabels[+i];
      return keepLink(referenceLink(codeText[+i], code, context) || code);
    }).replace(RE_REFERENCE, linkCandidate);
  };
  const parts = [], stack = [];
  let start = 0, open = -1;
  for (let i = 0; i < s.length; i++) {
    if (s[i] === '(' || s[i] === '（') {
      if (!stack.length) open = i;
      stack.push(s[i] === '(' ? ')' : '）');
    } else if (stack.length && s[i] === stack[stack.length - 1]) {
      stack.pop();
      if (!stack.length) {
        parts.push(s.slice(start, open + 1), parenthesized(s.slice(open + 1, i), s[open - 1] === ']'), s[i]);
        start = i + 1;
      }
    }
  }
  parts.push(s.slice(start));
  s = parts.join('');
  s = s.replace(/\u0000CODE(\d+)\u0000/g, (raw, i) =>
    keepLink(referenceLink(codeText[+i], codeLabels[+i], context) || codeLabels[+i]));
  return emphasis(esc(s))
    .replace(/\u0000LINK(\d+)\u0000/g, (_, i) => links[+i] || '')
    .replace(/\u0000CODE(\d+)\u0000/g, (_, i) => codeSpans[+i] || '')
    .replace(/\u0000IMG(\d+)\u0000/g, (_, i) => images[+i] || '');
}

// ---------------------------------------------------------------- 事件
function renderView() { SessionDockShell.updateView(S.view, S.nest); }

// A full render resolves filtering and hidden-parent successors. Folding only
// changes the visible rows within that same display tree, never its membership.
function sidebarNestContext() {
  return JSON.stringify([S.view, S.nest, S.picking, S.nestAttachUids, S.term, S.opts,
    S.activeOnly, [...S.off], HUB_MODE ? [...Nodes.off] : []]);
}

function patchNestFold(uid) {
  const side = $('#side'), tree = side?._nestTree;
  if (sidebarTextSelectionProtected() || !tree || tree.sessions !== S.sessions || tree.results !== S.results || tree.context !== sidebarNestContext()) return false;
  const node = side.querySelector(`.item[data-uid="${CSS.escape(uid)}"]`), current = node?._nestRow, group = node?.closest('.group');
  const index = group?._rows?.indexOf(current) ?? -1;
  if (!current || index < 0 || !node.querySelector('.nest-caret')) return false;
  const top = side.scrollTop, rows = [];
  if (sidebarNestClosed(uid)) rows.push({...current, closed: true});
  else expandRows(current.s, current.depth, tree.children, rows, new Set([uid]), new Map());
  let end = index + 1;
  while (end < group._rows.length && group._rows[end].depth > current.depth) end++;
  const delta = rows.length - 1 - (end - index - 1);
  const updated = group._rows.slice(0, index).concat(rows, group._rows.slice(end));
  let depth = current.depth;
  for (let i = index - 1; depth > 0 && i >= 0; i--) {
    const ancestor = updated[i]; if (ancestor.depth >= depth || ancestor.agent) continue;
    depth = ancestor.depth; updated[i] = {...ancestor, kids: ancestor.kids + delta};
  }
  SessionDockSidebar.updateGroups(SessionDockSidebar.currentGroups().map(view => view.key === group.dataset.key ? sidebarGroupView(view.key, updated) : view));
  stampSidebarGroups();
  const paths = rows.slice(1).flatMap(row => [...side.querySelectorAll(`.item[data-key="${CSS.escape(rowKey(row))}"] .cwd-path`)]);
  if (paths.length) fitTimelineDirectories(paths, timelineFitContext);
  side.scrollTop = top; return true;
}

/** Change only the clicked subtree; changed data or filters use the full path. */
function toggleNestFold(uid) {
  const folds = sidebarNestFolds();
  folds.has(uid) ? folds.delete(uid) : folds.add(uid);
  if (!S.term) store.set('nestClosed', [...S.nestClosed]);
  if (!patchNestFold(uid)) renderSide();
}

const SIDE_DEFAULT = SessionDockShell.SIDE_DEFAULT;
const sideResourceExtra = SessionDockShell.sideResourceExtra;
function setSideWidth(px, save) { SessionDockShell.setSideWidth(px, save); }
function setSideCollapsed(collapsed, save = true) { SessionDockShell.setSideCollapsed(collapsed, save); }

function cancelSearch(...args) { return SessionDockSearch.cancelSearch(...args); }
function searchProgress(...args) { return SessionDockSearch.searchProgress(...args); }
function searchProgressDone(...args) { return SessionDockSearch.searchProgressDone(...args); }
function showSearchMatches(...args) { return SessionDockSearch.showSearchMatches(...args); }
function fetchSearch(...args) { return SessionDockSearch.fetchSearch(...args); }
function runSearch(...args) { return SessionDockSearch.runSearch(...args); }
function renderOpts(...args) { return SessionDockSearch.renderOpts(...args); }

const appDisplayMode = SessionDockShell.displayMode;
function syncPageReload() { SessionDockShell.syncPageReload(); }
syncPageReload();

/* ---------- 回收站 ---------- */
// 删除只是把会话文件移进 ~/.local/share/sessiondock/trash/，这里是它唯一的出口：
// 看还剩什么、放回原处、或者真的删掉。




function openTrash(...args) {return SessionUiApp.openTrash(...args);}

function loadTrash(...args) {return SessionUiApp.loadTrash(...args);}












function purgeAllTrash(...args) {return SessionUiApp.purgeAllTrash(...args);}







// 终端后端是每台机器的服务端设置，不进 localStorage：换个浏览器看到的必须是同一份。
// 机器的名称、配色和控制台渲染都保存在中央服务端；接入和移除机器仍是服务器操作。
// 非 hub 的单机没有注册表，"本机"的控制台渲染存在这个浏览器里。
function machineTargets() {
  if (typeof T === 'undefined' || !T.listLoaded) return [];
  if (!HUB_MODE) {
    return T.enabled
      ? [{id: '', name: '本机', color: '', online: true, local: true, enabled: true,
          renderer: localConsoleRenderer()}] : [];
  }
  // 按注册表顺序列全部机器，停用的原位留着（只剩勾选框能把它接回来），不往后挪
  const machines = Nodes.machines.length ? Nodes.machines : Nodes.list;
  return machines.map(node => {
    const on = node.enabled !== false;
    const cap = on ? Nodes.capabilities[node.id] || {} : {};
    return {id: node.id, name: node.name, color: node.color || '',
            online: on ? node.online : null, local: false, enabled: on,
            terminal: !!cap.enabled, renderer: node.renderer === 'xterm' ? 'xterm' : 'grid'};
  });
}

// Machine contents and state have a single Vue owner. These named hooks remain
// available to terminal/list refresh callers and the existing settings tab shell.
function setMachineNote(text, isError = false) {
  SessionDockMachines.setMachineNote(text, isError);
}
function renderMachineSettings() {
  SessionDockMachines.renderMachineSettings();
}
function renderClientMatrix() {
  SessionDockMachines.renderClientMatrix();
}

const CONSOLE_RENDERERS = [['grid', '服务端网格（默认）'], ['xterm', 'xterm.js（浏览器解析）']];
function localConsoleRenderer() {
  return store.get('consoleRenderer', 'grid') === 'xterm' ? 'xterm' : 'grid';
}
// 控制台粘贴文件：默认关；开启后粘贴的文件存入会话目录并把路径填入终端（term.js）。
function consolePasteFilesEnabled() {
  return store.get('consolePasteFiles', false) === true;
}

function openSettings() { SessionDockSettings.open(); }

function settingsValues() {
  return {
    scale: interfaceScale(), font: store.get('font', 'ubuntu'),
    theme: store.get('theme', 'system'), cache: cacheLimitMb,
    sleep: SessionDockSleep.minutes, stopConcurrency: sessionStopConcurrency(),
    pasteFiles: consolePasteFilesEnabled(),
  };
}

// Called once by page-sleep.js after its existing service is initialized.
function mountSettings() {
  SessionDockShell.takeOverResourceToggle($('#sidebar-resources-toggle').onclick);
  $('#sidebar-resources-toggle').onclick = null;
  SessionDockMachines.mount({
    readTargets: machineTargets,
    appUrl,
    sourceNames: SOURCES,
    sources: Object.keys(SESSIONDOCK_CLIS),
    rendererOptions: CONSOLE_RENDERERS,
    offlineReason: target => typeof nodeOfflineReason === 'function'
      ? nodeOfflineReason(Nodes.list.find(n => n.id === target.id) || {}) : '离线',
    setLocalRenderer: value => store.set('consoleRenderer', value),
    applyDisplay: (target, changed) => {
      const node = Nodes.list.find(n => n.id === target.id);
      if (node) Object.assign(node, {name: changed.name, color: changed.color});
    },
    applyRenderer: (target, renderer) => {
      const node = [...Nodes.machines, ...Nodes.list].find(n => n.id === target.id);
      if (node) node.renderer = renderer;
    },
    applyOrder: machines => { Nodes.machines = machines; },
    loadNodes,
    loadSessions: () => loadSessions(true),
    refreshLive: () => refreshLive(true),
    loadTermList: () => typeof loadTermList === 'function' ? loadTermList() : null,
    renderNodes: () => { if (typeof renderNodes === 'function') renderNodes(); },
  });

  SessionDockSettings.mount({
    read: settingsValues,
    scale: applyInterfaceScale,
    scaleIndicator: showScaleIndicator,
    font: value => applyFont(value, true),
    theme: value => applyTheme(value, true),
    sleep: value => SessionDockSleep.configure(value, true),
    cache: value => {
      cacheLimitMb = Math.max(0, +value || 0);
      CACHE_MAX_BYTES = cacheLimitMb ? cacheLimitMb * 1024 * 1024 : Infinity;
      store.set('cacheMb', cacheLimitMb);
      trimCache();
      SessionDockSettings.update({cache: cacheLimitMb});
    },
    stopConcurrency: value => {
      store.set('stopConcurrency', Number(value));
      SessionDockSettings.update({stopConcurrency: sessionStopConcurrency()});
    },
    pasteFiles: value => {
      store.set('consolePasteFiles', value === true);
      SessionDockSettings.update({pasteFiles: consolePasteFilesEnabled()});
    },
    pwa: SessionDockPwaInstall,
  }, {read: () => store.get('settingsTab', 'appearance'), save: value => store.set('settingsTab', value)});
  settingsMounted = true;
}

document.addEventListener('keydown', e => {
  if (e.key !== 'Escape') return;
  if (globalThis.SessionDockGroups?.escapeMenu()) return;
  if (!$('#item-menu').hidden) return closeItemMenu();
  if (S.nestAttach) return setNestAttach('');
  if (S.picking) return setPicking(false);
  $('#q').blur();
});

// SessionDock 是前身服务的替代品而非开发版：没有常驻横幅。只有能力声明
// 本身无法解析（capabilities.js 失效关闭）时才提示检查服务配置。
const backendNotice = $('#backend-notice');
if (backendNotice && SessionDockCapabilities.config.configuration_error) {
  backendNotice.textContent = '能力配置无效，请检查服务配置。';
  backendNotice.hidden = false;
}
const SessionUiApp = SessionDockSessionUi.createAppControllers(
{$: (...args) => $(...args),
  get ConsoleUI() {return ConsoleUI;},
  get HUB_MODE() {return HUB_MODE;},
  get MEDIUM() {return MEDIUM;},
  get MOBILE() {return MOBILE;},
  get S() {return S;},
  get SOURCES() {return SOURCES;},
  get SessionDockCapabilities() {return SessionDockCapabilities;},
  get SessionDockShell() {return SessionDockShell;},
  get T() {return typeof T === 'undefined' ? undefined : T;},
  appAlert: (...args) => appAlert(...args),
  appConfirm: (...args) => appConfirm(...args),
  appUrl: (...args) => appUrl(...args),
  auditDetailRendered: (...args) => auditDetailRendered(...args),
  browserAuditEvent: (...args) => browserAuditEvent(...args),
  get cache() {return cache;},
  cloneSessionGroup: (...args) => cloneSessionGroup(...args),
  closeWatch: (...args) => closeWatch(...args),
  consoleUnavailableReason: (...args) => consoleUnavailableReason(...args),
  deletePendingSession: (...args) => deletePendingSession(...args),
  ensureConsolePlaceholder: (...args) => ensureConsolePlaceholder(...args),
  fmtSize: (...args) => fmtSize(...args),
  fmtSpan: (...args) => fmtSpan(...args),
  fmtTime: (...args) => fmtTime(...args),
  forgetDeletedReceipts: (...args) => forgetDeletedReceipts(...args),
  forkAncestors: (...args) => forkAncestors(...args),
  forkChildren: (...args) => forkChildren(...args),
  layoutHeader: (...args) => layoutHeader(...args),
  layoutTier: (...args) => layoutTier(...args),
  linkedTermSession: (...args) => linkedTermSession(...args),
  loadTermList: (...args) => loadTermList(...args),
  nodeColor: (...args) => nodeColor(...args),
  openSession: (...args) => openSession(...args),
  paintConsoleAvailability: (...args) => paintConsoleAvailability(...args),
  paintLive: (...args) => paintLive(...args),
  paintTransferAvailability: (...args) => paintTransferAvailability(...args),
  paintTurn: (...args) => paintTurn(...args),
  post: (...args) => post(...args),
  refreshLive: (...args) => refreshLive(...args),
  refreshMessageTimeDividers: (...args) => refreshMessageTimeDividers(...args),
  renderChips: (...args) => renderChips(...args),
  renderSide: (...args) => renderSide(...args),
  renderTakeoverBtn: (...args) => renderTakeoverBtn(...args),
  sessionFrozen: (...args) => sessionFrozen(...args),
  sessionIconMarkup: (...args) => sessionIconMarkup(...args),
  get sessionStopBusy() {return sessionStopBusy;},
  get sessionStopProgress() {return sessionStopProgress;},
  sessionTurn: (...args) => sessionTurn(...args),
  setForkParentVisibility: (...args) => setForkParentVisibility(...args),
  settle: (...args) => settle(...args),
  shortCwd: (...args) => shortCwd(...args),
  showConsoleToast: (...args) => showConsoleToast(...args),
  showMobileList: (...args) => showMobileList(...args),
  get store() {return store;},
  takeover: (...args) => takeover(...args),
  toggleLinkedTermSession: (...args) => toggleLinkedTermSession(...args),
  toggleSessionStar: (...args) => toggleSessionStar(...args),
  viewKey: (...args) => viewKey(...args),
  watchSession: (...args) => watchSession(...args),
  auditHeaderLayout: (...args) => auditHeaderLayout(...args),
  renderPendingSessionAction: (...args) => renderPendingSessionAction(...args)},
{$: (...args) => $(...args),
  get HUB_MODE() {return HUB_MODE;},
  get SOURCES() {return SOURCES;},
  appConfirm: (...args) => appConfirm(...args),
  appUrl: (...args) => appUrl(...args),
  applyNodeState: (...args) => applyNodeState(...args),
  cancelSearch: (...args) => cancelSearch(...args),
  fmtSize: (...args) => fmtSize(...args),
  fmtTime: (...args) => fmtTime(...args),
  icon: (...args) => icon(...args),
  loadSessions: (...args) => loadSessions(...args),
  nodeDirectory: (...args) => nodeDirectory(...args),
  selectedNodeIds: (...args) => selectedNodeIds(...args),
  shortCwd: (...args) => shortCwd(...args),
  trashCapable: (...args) => trashCapable(...args)},
{$: (...args) => $(...args),
  get HUB_MODE() {return HUB_MODE;},
  get Nodes() {return Nodes;},
  get SOURCES() {return SOURCES;},
  get SessionDockCapabilities() {return SessionDockCapabilities;},
  get SessionDockShell() {return SessionDockShell;},
  appUrl: (...args) => appUrl(...args),
  closeSessionActions: (...args) => closeSessionActions(...args),
  fmtSize: (...args) => fmtSize(...args),
  loadSessions: (...args) => loadSessions(...args),
  nodeOf: (...args) => nodeOf(...args),
  openSession: (...args) => openSession(...args),
  sessionStoppable: (...args) => sessionStoppable(...args),
  setControlUnavailable: (...args) => setControlUnavailable(...args),
  showSessionStopNotice: (...args) => showSessionStopNotice(...args),
  sidebarSessions: (...args) => sidebarSessions(...args)});
SessionDockShell.mount(shellBridge);
renderOpts();
renderPickBar();
renderView();
ensureConsolePlaceholder();
pollLive();   // 终端面板由 term.js 自己初始化 (它在本文件之后加载)
function uidOfDeepLink(spec) {
  if (!spec) return null;
  const cut = spec.indexOf(':');
  const source = cut > 0 ? spec.slice(0, cut) : null;
  const sid = cut > 0 ? spec.slice(cut + 1) : spec;
  const matches = S.sessions.filter(s => s.sid === sid && (!source || s.source === source) && (!deepNode() || s.node_id === deepNode()));
  const byUid = new Map(matches.map(row => [row.uid, row]));
  // Verified rollout generations share one native ID. Collapse only explicit
  // continuations within this identity and machine; independent copies remain ambiguous.
  const current = new Set(matches.map(row => {
    const seen = new Set();
    while (row.continued_in) {
      if (seen.has(row.uid)) return null;
      seen.add(row.uid);
      const next = byUid.get(row.continued_in);
      if (!next || next.source !== row.source || (next.node_id || '') !== (row.node_id || '')) break;
      row = next;
    }
    return row.uid;
  }));
  const hit = (matches.length === 1 ? matches[0]
    : current.size === 1 ? byUid.get(current.values().next().value) : null)
    || S.sessions.find(s => s.uid === spec);
  return hit ? hit.uid : null;
}
function agentOfDeepLink(spec) {
  if (!spec) return null;
  const cut=spec.indexOf(':');
  const source=cut>0 ? spec.slice(0,cut) : null;
  const id=cut>0 ? spec.slice(cut+1) : spec;
  const matches=S.sessions.filter(s=>(!source || s.source===source) && (!deepNode() || s.node_id===deepNode()))
    .flatMap(s=>(s.agent_items || []).filter(a=>a.id===id).map(a=>({uid:s.uid,agent:a.id})));
  return matches.length===1 ? matches[0] : null;
}
function routeSession(spec) {
  const marker = spec.indexOf('/agent:');
  if (marker >= 0) {
    const uid = uidOfDeepLink(spec.slice(0, marker));
    const id = spec.slice(marker + 7);
    const item = S.sessions.find(s => s.uid === uid)?.agent_items?.find(a => a.id === id || a.id === 'agent-' + id);
    return uid && item ? {uid, agent: item.id} : null;
  }
  const uid = uidOfDeepLink(spec);
  return uid ? {uid, agent: null} : agentOfDeepLink(spec);
}
addEventListener('popstate', () => {
  const route = routeSession(new URL(location.href).searchParams.get('sid') || '');
  if (route) openSession(route.uid, route.agent, {exact: true, historyMode: 'none'});
});
// index.html 在首帧前就按上次停留的页面进了手机会话页；列表回来前先占位，
// 恢复不了（列表失败、会话已不在）时退回列表，不写 mobilePage，下次刷新照旧恢复。
if (document.body.classList.contains('mobile-detail') && !S.sel) {
  $('#detail').innerHTML = '<div class="spin">正在读取会话…</div>';
}
function leaveBootDetail() {
  if (S.sel || !document.body.classList.contains('mobile-detail')) return;
  SessionDockShell.leaveBootDetail();
  $('#detail').innerHTML = '<div class="empty">从左侧选择一个会话</div>';
  layoutHeader();
}
loadSessions(false).then(async ok => {
  if (!ok) return leaveBootDetail();
  const route = routeSession(DEEP_SID);
  if (route) { openSession(route.uid, route.agent, {exact: true, historyMode: 'replace'}); return; }
  const last = store.get('sel', null);       // 恢复上次看的会话
  const savedAgent = store.get('agent', null);
  const restoreDetail = !MOBILE.matches || store.get('mobilePage', 'list') === 'detail';
  if (restoreDetail && last && S.sessions.some(s => s.uid === last)) {
    openSession(last, savedAgent?.uid === last ? savedAgent.id : null);
  } else if (restoreDetail && last?.startsWith('tmux:')) {
    // New launches have no native history yet. Their durable identity comes
    // from term/list, which can arrive after the native catalog on reload.
    // app.js can receive its catalog before the later deferred term.js runs.
    if (typeof T === 'undefined') await new Promise(resolve =>
      document.addEventListener('DOMContentLoaded', resolve, {once: true}));
    let request = T.listRequest || loadTermList();
    do {
      await request;
      // Live polling may supersede an earlier request before it completes.
      request = T.listRequest;
    } while (request);
    // A slow list must not undo navigation performed while it was loading.
    if (S.sel || store.get('sel', null) !== last
        || (MOBILE.matches && store.get('mobilePage', 'list') !== 'detail')) return;
    // Use the receipt even if its native binding just appeared: the normal
    // pending opener follows that binding and migrates the saved draft.
    const pending = T.pending.find(row => pendingUid(row.name) === last)
      || pendingTmuxSessions().find(row => row.uid === last);
    if (pending) await openPendingSession(pending);
    else leaveBootDetail();
  } else leaveBootDetail();
});


/** Whole-group transfer preview. A confirmed local clone keeps its operation ID
 * across uncertain responses; unsupported selections never use the local API. */
function transferUnavailableReason(...args) {return SessionUiApp.transferUnavailableReason(...args);}
function paintTransferAvailability(...args) {return SessionUiApp.paintTransferAvailability(...args);}
function cloneSessionGroup(...args) {return SessionUiApp.cloneSessionGroup(...args);}


function transferPhaseLabel(...args) {return SessionUiApp.transferPhaseLabel(...args);}

function refreshTransferTasks(...args) {return SessionUiApp.refreshTransferTasks(...args);}
function openTransferTasks(...args) {return SessionUiApp.openTransferTasks(...args);}


function sidebarVueGestures() {return {mousedown: event => {sidebarGestureMousedown1(event);},
pointerdown: event => {sidebarGesturePointerdown1(event); sidebarGesturePointerdown2(event);},
pointermove: event => {sidebarGesturePointermove1(event);},
pointerup: event => {cancelLongPress();},
pointercancel: event => {cancelLongPress();},
pointerleave: event => {if (!$('#side').contains(event.relatedTarget)) cancelLongPress();},
contextmenu: event => {sidebarGestureContextmenu1(event);},
clickCapture: event => {sidebarGestureClick1(event); sidebarGestureClick2(event); sidebarGestureClick3(event);},
selectstart: event => {sidebarGestureSelectstart1(event);}};
}

// The pending-session action service still lives in term.js. Its one sidebar
// selection hook is installed after defer scripts load, before async boot opens it.
document.addEventListener('DOMContentLoaded', () => {
  selectPendingSidebarRow = (uid, added) => {
    if (added) renderSide(); else paintSidebarSelection(uid);
  };
});
SessionDockConversation.configure({
  md, head, renderFormulae, paintSyntax, clearSyntaxPaint, paintToolOutputDiff,
  retryableReadFailure, retryMigrationRead,
  safeMediaSrc, lazyMediaEnabled, mediaContinuationEnabled, mediaMoreInfo, diagnoseMedia,
  createView: captureView,
  forgetMediaDiagnostic,
  reloadMediaOwned: (view, notice) => reloadMediaSession(view, {}, {set textContent(value) {notice(value);}}),
  loadMedia: (cursor, button) => loadMediaContinuation(S.sel,S.agent,cursor,button),
  loadHistory: (info,button) => historyPagesEnabled() ? loadHistoryPage(info.uid,info.agent,button) : loadFullHistory(info.uid,info.agent,button),
  reloadHistory: (info,button) => reloadHistoryWindow(info.uid,info.agent,button),
  sticking: () => _stick, live: uid => S.live.has(uid), settle,
  mutateKeepingMessageAnchor, jumpWithinConversation, uiIcon,
  sessiondockCli, answer: (...args) => answerCliQuestion(...args),
  cancel: (...args) => cancelCliQuestion(...args),
  answerCliQuestionForm: (...args) => answerCliQuestionForm(...args),
  revealNativeTerminal: uid => revealNativeTerminal(uid),
  dismissQueuedSend: (...args) => dismissQueuedSend(...args),
  pinTimeline, pinTarget: m => {
    if (!timelinePinEnabled() || m.role !== 'user' || m.turn_id == null || S.agent) return null;
    const session = S.sessions.find(s => s.uid === S.sel) || cache.get(viewKey(S.sel,null))?.meta;
    return session?.source === 'claude' ? String(m.turn_id) : null;
  },
  questionDraft: (m,rows) => {
    const key=m.uid && m.call_id ? `${m.uid}\0${m.call_id}` : '';
    let value=key ? questionFormDrafts.get(key) : null;
    if (!Array.isArray(value) || value.length!==rows.length || !value.every((n,i)=>n===null || (Number.isInteger(n) && !!rows[i]?.options?.[n]))) {value=Array(rows.length).fill(null);if(key)questionFormDrafts.set(key,value);}
    return value;
  },
  screenText: m => screenMenuTextDrafts.get(`${composerDraftOwner(m.uid)}\0${m.id}`),
  saveScreenText: (m,value) => {
    const owner=composerDraftOwner(m.uid),key=`${owner}\0${m.id}`;
    for(const saved of screenMenuTextDrafts.keys())if(saved.startsWith(`${owner}\0`) && saved!==key)screenMenuTextDrafts.delete(saved);
    screenMenuTextDrafts.set(key,value);
  },
  hasTerm, searchCanOpen: () => S.autoOpen < AUTO_OPEN_MAX,
  claimSearchOpen: () => {if (S.autoOpen >= AUTO_OPEN_MAX) return false;S.autoOpen++;return true;},
  searchMessage: text => {const found=hasTerm(text),open=found && S.autoOpen < AUTO_OPEN_MAX;if(open)S.autoOpen++;return {found,open};},
  searchAsync: (text,apply) => {
    if (!S.term || !S.opts.regex || hasTerm(text)) return;
    const generation=SessionDockSearch.generation();
    regexMatches(text).then(ranges => {if(generation===SessionDockSearch.generation() && ranges.matched)apply();});
  },
  searchProcessAsync: (items,apply) => {
    if (!S.term || !S.opts.regex || items.some(m => SEARCH_ROLES.has(m.role) && hasTerm(m.text))) return;
    const generation=SessionDockSearch.generation();
    Promise.all(items.filter(m => SEARCH_ROLES.has(m.role)).map(m => regexMatches(m.text))).then(results => {if(generation===SessionDockSearch.generation() && results.some(r=>r.matched))apply();});
  },
  markInner: root => {if(S.term){markMatches(root);updateMatchNav();}},
});

SessionDockConversation.pages.configurePages(mediaMoreInfo);

function opencodeDeleteNote(...args) {return SessionUiApp.opencodeDeleteNote(...args);}

function trashLocationNote(...args) {return SessionUiApp.trashLocationNote(...args);}

function trashCapable(...args) {return SessionUiApp.trashCapable(...args);}

function sessionStopCapable(...args) {return SessionUiApp.sessionStopCapable(...args);}



globalThis.showConsoleToast = reason => SessionDockSessionUi.consoleToast(reason);
